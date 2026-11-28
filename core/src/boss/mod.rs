// The boss: serves the web ui and the API, owns the history in the db, and decides what runs where.
// This module holds the boss itself, its router, and what every part of it uses; runs, workers,
// users and artifacts each have a module of their own.

use crate::{
    auth, client,
    db::{self, Db},
    lock, passkey, pipedef,
    repo::Repo,
    tls, workdir,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Path as AxumPath, State},
    http::{HeaderMap, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bettertest_shared::*;
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    convert::Infallible,
    error::Error,
    fs,
    io::ErrorKind,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::AsyncWriteExt,
    net::TcpListener,
    spawn,
    sync::{Notify, broadcast, mpsc, watch},
    task::{AbortHandle, JoinSet},
    time::{sleep, timeout},
};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

mod artifacts;
mod runs;
mod users;
mod workers;

use artifacts::*;
use runs::*;
use users::*;
use workers::*;

// frontend/dist, with the brotli and gzip copies build.rs made of it.
#[derive(Embed)]
#[folder = "$OUT_DIR/web/"]
struct EmbeddedWebAssets;

pub const LISTEN_PORT: u16 = 7357;

struct Boss {
    // Pipeline name (the pipedef's file name without .py) to where its pipedef is. Repos add the
    // pipedefs their default branch has whenever they are fetched.
    pipedefs: Mutex<BTreeMap<String, Pipedef>>,
    // By pipeline name, the source it last parsed and what parsing it gave, so that a run starts
    // python only when the pipedef changed. One per pipeline, whichever commit it was.
    parsed: Mutex<HashMap<String, (String, Pipeline)>>,
    // By pipeline name, the pipeline as its page shows it.
    shown: Mutex<HashMap<String, Shown>>,
    repos: Vec<Repo>,
    // Runs not finished yet, so a newer commit on the same branch can cancel them.
    going: Mutex<HashMap<Uuid, Going>>,
    db: Mutex<Db>,
    // By url. Changed only while holding `db` too (see change_workers), so that snapshots of them
    // are gapless like everything else.
    workers: Mutex<BTreeMap<String, Watched>>,
    // The hashes of the tokens of tasks running now, which may read from the boss (their checkout,
    // artifacts) whatever logged out may do.
    task_tokens: Mutex<HashMap<String, Uuid>>,
    // Passkey ceremonies under way, by the id the boss handed out for each.
    ceremonies: Mutex<HashMap<String, Pending>>,
    events: broadcast::Sender<Event>,
    // Woken whenever a worker changes, or a task stops waiting for one, which is when a task waiting
    // for a busy worker may get one.
    freed: Notify,
    // The tasks waiting for a busy worker, by run order (see Db::run_order) and task, with the
    // worker or labels each needs. A freed slot goes to the oldest run that can use it.
    waiting: Mutex<BTreeMap<(i64, Uuid), Wants>>,
    http: reqwest::Client,
    // The certificate the boss presents, when it speaks HTTPS.
    tls: Option<tls::Tls>,
    // Where the web UI and API listen. Workers call back here: they take the IP from the
    // start-task connection and this port from that same form.
    port: u16,
}

/// A pipeline as its page shows it, the default branch's or the file as it is now: parsed, and kept
/// in memory, so that a page load starts no python and reads no pipedef. A repo's are parsed again
/// whenever it is fetched. A file's are parsed again once the file changed: `stamp` is when it was
/// modified, and its size, as of the parse, which a page load compares with the file's (a stat).
#[derive(Clone)]
struct Shown {
    stamp: Option<(SystemTime, u64)>,
    pipeline: Result<Pipeline, String>,
}

/// Where a pipeline's pipedef is.
#[derive(Clone)]
enum Pipedef {
    File(PathBuf),
    // A file in `.bettertest/` of repo number `repo`, read from whichever commit a run tests.
    Repo { repo: usize, path: String },
}

type Shared = Arc<Boss>;

/// Each source is a pipedef file, a directory whose `*.py` files are pipedefs (read once, at
/// startup), or the url of a git repo whose `.bettertest/*.py` files are pipedefs. A repo url may
/// carry credentials, `http://user:token@host/owner/name.git`, for fetching and for reporting
/// commit statuses.
/// With `tls`, the boss speaks HTTPS only. `port` is where it listens (`--boss-port`, 7357 unless
/// told otherwise). `local_worker`, when `unified` started a worker in this process, is that
/// worker's url; it is enrolled with the others.
pub async fn entry(
    sources: Vec<String>,
    tls: Option<tls::Tls>,
    port: u16,
    local_worker: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let mut pipedefs = vec![];
    let mut repos = vec![];
    for source in sources {
        if source.starts_with("http://") || source.starts_with("https://") {
            repos.push(Repo::new(&source)?);
            continue;
        }
        let path = PathBuf::from(source);
        if !path.is_dir() {
            pipedefs.push(path);
            continue;
        }
        for entry in fs::read_dir(&path)? {
            let file = entry?.path();
            if file.extension().is_some_and(|extension| extension == "py") {
                pipedefs.push(file);
            }
        }
    }

    let mut names = BTreeMap::new();
    for path in pipedefs {
        let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            return Err(format!("{} is not a file", path.display()).into());
        };
        if !valid_pipeline_name(&name) {
            let path = path.display();
            return Err(
                format!("rename {path}: pipedef names are letters, digits, '-' and '_'").into(),
            );
        }
        if let Some(Pipedef::File(other)) = names.insert(name.clone(), Pipedef::File(path.clone()))
        {
            let (a, b) = (other.display(), path.display());
            return Err(format!("{a} and {b} would both be pipeline {name}").into());
        }
    }

    let db = db::open()?;
    // Runs the previous boss process left going, cut short when it stopped. They go on.
    let interrupted = db.running_runs()?;
    let enrolled = db.workers()?;

    // Big enough that only a badly stuck browser falls behind. One that does gets a fresh snapshot.
    let (events, _) = broadcast::channel(4096);
    let boss = Arc::new(Boss {
        pipedefs: Mutex::new(names),
        parsed: Mutex::new(HashMap::new()),
        shown: Mutex::new(HashMap::new()),
        repos,
        going: Mutex::new(HashMap::new()),
        db: Mutex::new(db),
        workers: Mutex::new(BTreeMap::new()),
        task_tokens: Mutex::new(HashMap::new()),
        ceremonies: Mutex::new(HashMap::new()),
        events,
        freed: Notify::new(),
        waiting: Mutex::new(BTreeMap::new()),
        http: client::http()?,
        tls,
        port,
    });
    for url in enrolled {
        boss.enroll(&url);
    }
    // Unified's own worker. enroll does nothing when this url is already known, so a restart, or
    // adding the same url on the workers page, leaves one row.
    if let Some(url) = local_worker {
        boss.enroll(url);
    }
    spawn(sweep_hourly(boss.clone()));
    for index in 0..boss.repos.len() {
        boss.fetch(index).await;
    }
    let resuming = interrupted.len();
    if resuming > 0 {
        println!("resuming {resuming} runs a previous boss left going");
    }
    for id in interrupted {
        spawn(resume(boss.clone(), id, "the boss restarted".into(), None));
    }
    if boss.pipelines().is_empty() {
        println!("no pipedefs yet. add some and restart the boss");
    }
    // Parsed now, the first page load of each pipeline needs no python either. Fetching parsed
    // the repos' already.
    let parsing = boss.clone();
    spawn(async move {
        for name in parsing.pipelines() {
            let _ = shown_pipeline(&parsing, &name).await;
        }
    });

    // The router takes the boss along; this keeps hold of its certificate for serving.
    let serving = boss.clone();
    let router = Router::new()
        .route("/api/pipelines", get(get_pipelines))
        .route("/api/pipelines/events", get(pipelines_events))
        .route("/api/pipeline/{name}", get(get_pipeline))
        .route("/api/pipeline/{name}/events", get(pipeline_events))
        .route("/api/start-run/{name}", post(start_run))
        .route("/api/push-hook", post(push_hook))
        .route("/api/cancel-run/{run}", post(cancel_run))
        .route("/api/restart/{*restart}", post(restart))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/me", get(get_me))
        .route("/api/startup-error", post(startup_error))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/users", get(get_users))
        .route("/api/add-user", post(add_user))
        .route("/api/set-role", post(set_role))
        .route("/api/set-password", post(set_password))
        .route("/api/remove-user", post(remove_user))
        .route("/api/https", get(get_https))
        .route("/api/new-token", post(new_token))
        .route("/api/revoke-tokens", post(revoke_tokens))
        .route("/api/new-certificate", post(new_certificate))
        .route("/api/revoke-credential", post(revoke_credential))
        .route("/api/new-passkey-options", post(new_passkey_options))
        .route("/api/new-passkey", post(new_passkey))
        .route("/api/passkey-login-options", post(passkey_login_options))
        .route("/api/passkey-login", post(passkey_login))
        .route("/api/workers", get(get_workers))
        .route("/api/workers/events", get(workers_events))
        .route("/api/add-worker", post(add_worker))
        .route("/api/forget-worker", post(forget_worker))
        .route("/api/run/{id}", get(get_run))
        .route("/api/run/{id}/events", get(run_events))
        .route("/api/run/{id}/source.tar", get(get_source))
        .route("/api/run/{id}/views", get(get_views))
        .route("/api/task/{id}/log", get(get_task_log))
        .route("/api/task/{id}/events", get(task_events))
        .route("/api/task/{id}/artifact/{name}", get(get_task_artifact))
        .route(
            "/api/run/{id}/artifact/{stage}/{task}/{name}",
            get(get_run_artifact),
        )
        .route(
            "/api/pipeline/{name}/run/{run}/artifact/{stage}/{task}/{artifact}",
            get(get_pipeline_artifact),
        )
        .route("/artifact/{name}", get(get_artifact).put(put_artifact))
        // The homepage's "Try now": one curl|sh, then these bytes. The files themselves are copied
        // into place at deploy (see DL_DIR); nothing here writes them.
        .route("/install.sh", get(install_sh))
        .route("/dl/{name}", get(download_binary))
        // Pages of the web ui. The frontend reads the path and decides what to show.
        .route("/", get(home))
        .route("/pipeline/{*rest}", get(index))
        .route("/task/{*rest}", get(index))
        .route("/workers", get(index))
        .route("/settings", get(index))
        .route("/users", get(index))
        .route("/login", get(index))
        .route("/{*path}", get(get_embedded_asset))
        .layer(middleware::from_fn_with_state(boss.clone(), guard))
        .with_state(boss);

    let listener = TcpListener::bind(("::", port)).await?;
    let service = router.into_make_service_with_connect_info::<tls::Peer>();
    match &serving.tls {
        Some(tls) => {
            println!("boss web ui at https://{}:{port}", tls.name);
            let names = tls.names.join(", ");
            println!("certificate for {names}, expires {}", tls.expires);
            axum::serve(tls::listen(listener, tls), service).await?;
        }
        None => {
            println!("boss web ui at http://[::1]:{port}");
            axum::serve(tls::PlainListener(listener), service).await?;
        }
    }
    Ok(())
}

pub(crate) type Failure = (StatusCode, String);

/// Milliseconds since 1970. A clock set before 1970 reads as 1970.
fn now() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    since.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

pub(crate) fn internal(e: impl ToString) -> Failure {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub(crate) fn not_found(what: impl std::fmt::Display) -> Failure {
    (StatusCode::NOT_FOUND, format!("no {what}"))
}

pub(crate) fn bad_request(why: impl ToString) -> Failure {
    (StatusCode::BAD_REQUEST, why.to_string())
}

pub(crate) fn conflict(why: impl ToString) -> Failure {
    (StatusCode::CONFLICT, why.to_string())
}

fn unauthorized(why: impl ToString) -> Failure {
    (StatusCode::UNAUTHORIZED, why.to_string())
}

fn parse_id(id: &str) -> Result<Uuid, Failure> {
    Uuid::parse_str(id).map_err(|_| bad_request(format!("{id:?} is not a uuid")))
}

/// Retention keeps up with the clock once an hour, which is plenty for settings counted in days.
async fn sweep_hourly(boss: Shared) {
    let mut hourly = tokio::time::interval(Duration::from_secs(60 * 60));
    loop {
        hourly.tick().await;
        if let Err(e) = boss.sweep(now()) {
            eprintln!("retention failed: {e}");
        }
    }
}

/// Whether the boss speaks HTTPS, and with which certificate. It is set by `--cert` and `--key` at
/// start, so this only reads.
async fn get_https(State(boss): State<Shared>) -> Json<Option<Https>> {
    Json(boss.tls.as_ref().map(|tls| Https {
        names: tls.names.clone(),
        expires: tls.expires.clone(),
        days_left: tls.days_left,
    }))
}

async fn get_settings(State(boss): State<Shared>) -> Result<Json<Settings>, Failure> {
    boss.db().settings().map(Json).map_err(internal)
}

/// Saves the settings and applies them at once, so what they no longer keep is gone by the time
/// this returns.
async fn put_settings(
    State(boss): State<Shared>,
    Json(settings): Json<Settings>,
) -> Result<Json<Swept>, Failure> {
    for days in [settings.artifact_days, settings.log_days].iter().flatten() {
        if !(1..=36500).contains(days) {
            let why = format!("keep things 1 to 36500 days, or forever, not {days} days");
            return Err(bad_request(why));
        }
    }
    boss.db().set_settings(&settings).map_err(internal)?;
    let (logs, artifacts) = boss.sweep(now()).map_err(internal)?;
    Ok(Json(Swept { logs, artifacts }))
}

/// An SSE stream that starts with a snapshot, then sends every event `wanted` keeps. A client too
/// slow to keep up gets a fresh snapshot rather than a gap. `snapshot` returning None means there
/// is no such thing to watch. Gzipped when the browser takes gzip (`headers`), which shrinks a big
/// snapshot about tenfold: each message is flushed through on its own, so none waits for the next.
fn live<S: Serialize + Send + 'static>(
    boss: Shared,
    headers: &HeaderMap,
    snapshot: impl Fn(&Db) -> rusqlite::Result<Option<S>> + Send + 'static,
    wanted: impl Fn(&S, &Event) -> bool + Send + 'static,
) -> Result<Response, Failure> {
    let (current, mut events) = boss.subscribe(&snapshot).map_err(internal)?;
    let Some(mut current) = current else {
        return Err((StatusCode::NOT_FOUND, "not found".into()));
    };
    let gzip = accepts_gzip(text_header(headers, header::ACCEPT_ENCODING));
    let mut wire = Wire::new(gzip);

    let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
    spawn(async move {
        loop {
            let message = sse_message(&Live::Snapshot(&current));
            if !send(&tx, &mut wire, &message).await {
                return;
            }
            loop {
                let received = tokio::select! {
                    _ = tx.closed() => return,
                    received = events.recv() => Some(received),
                    () = sleep(PING) => None,
                };
                let message = match received {
                    Some(Ok(event)) if wanted(&current, &event) => {
                        sse_message(&Live::<&S>::Event(event))
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(broadcast::error::RecvError::Lagged(_))) => break,
                    Some(Err(broadcast::error::RecvError::Closed)) => return,
                    // A named event rather than the usual comment, which browsers keep from the
                    // page: hearing nothing, not even this, is how the page tells a lost boss from
                    // a quiet one. It has data, since browsers drop an event without any.
                    None => b"event: ping\ndata: ping\n\n".to_vec(),
                };
                if !send(&tx, &mut wire, &message).await {
                    return;
                }
            }
            match boss.subscribe(&snapshot) {
                Ok((Some(fresh), receiver)) => (current, events) = (fresh, receiver),
                Ok((None, _)) => return,
                Err(e) => {
                    eprintln!("could not take a fresh snapshot: {e}");
                    return;
                }
            }
        }
    });

    let body = Body::from_stream(ReceiverStream::new(rx).map(Ok::<_, Infallible>));
    let sse = [
        (header::CONTENT_TYPE, "text/event-stream"),
        (header::CACHE_CONTROL, "no-cache"),
        (header::VARY, "accept-encoding"),
    ];
    let encoded = gzip.then_some([(header::CONTENT_ENCODING, "gzip")]);
    Ok((sse, encoded, body).into_response())
}

/// Sends `message` down a live stream, through `wire`. False once the browser is gone.
async fn send(tx: &mpsc::Sender<Vec<u8>>, wire: &mut Wire, message: &[u8]) -> bool {
    match wire.pack(message) {
        Some(bytes) => tx.send(bytes).await.is_ok(),
        None => false,
    }
}

/// How often a live stream says it is still there when there is nothing else to say.
const PING: Duration = Duration::from_secs(15);

/// One SSE message of `data` as JSON, on one line: serde_json writes no line breaks.
fn sse_message(data: &impl Serialize) -> Vec<u8> {
    // Serializing our own plain structs cannot fail. If it somehow did, an empty message is
    // harmless.
    let json = serde_json::to_vec(data).unwrap_or_default();
    [b"data: ".as_slice(), &json, b"\n\n"].concat()
}

/// The bytes of a stream as they go on the wire: as they are, or gzipped as one gzip stream for
/// the whole connection.
struct Wire(Option<flate2::write::GzEncoder<Vec<u8>>>);

impl Wire {
    fn new(gzip: bool) -> Self {
        // The fastest level: a snapshot is mostly ids and repeated names, which compress anyway.
        Wire(gzip.then(|| flate2::write::GzEncoder::new(vec![], flate2::Compression::fast())))
    }

    /// `message`'s bytes on the wire, all of them, so the browser can read it now. None if gzip
    /// failed, which ends the stream; the browser reconnects and starts over.
    fn pack(&mut self, message: &[u8]) -> Option<Vec<u8>> {
        use std::io::Write;
        let Some(gzip) = &mut self.0 else {
            return Some(message.to_vec());
        };
        // A sync flush: everything written so far comes out, and the stream goes on after it.
        gzip.write_all(message).ok()?;
        gzip.flush().ok()?;
        Some(std::mem::take(gzip.get_mut()))
    }
}

/// Pipeline names end up in URLs.
fn valid_pipeline_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// A browser that could not start the web ui says why, and the boss logs it: the page alone would
/// show it only to whoever is holding that browser.
async fn startup_error(headers: HeaderMap, body: String) {
    let agent = text_header(&headers, header::USER_AGENT);
    let said: String = body.chars().take(2000).collect();
    println!("a browser could not start the web ui ({agent}): {said:?}");
}

/// Musl binaries `install.sh` tells curl to fetch. Deploy copies them here (see deploy.sh); this
/// process only reads them. They are not 0666, unlike the db: a stranger pipes them into a shell,
/// so only the person who deploys can write them.
const DL_DIR: &str = "/var/tmp/bettertest/dl";

fn binary_file(name: &str) -> Option<PathBuf> {
    matches!(name, "bettertest-x86_64" | "bettertest-aarch64")
        .then(|| PathBuf::from(DL_DIR).join(name))
}

/// `x86_64` when the query says nothing. `aarch64` is the other accepted value. Anything else is
/// refused: the choice is written into a shell script.
fn download_arch(query: Option<&str>) -> Result<&'static str, &'static str> {
    let Some(query) = query.filter(|query| !query.is_empty()) else {
        return Ok("x86_64");
    };
    let mut chosen = None;
    for pair in query.split('&') {
        let Some(("arch", value)) = pair.split_once('=') else {
            continue;
        };
        if chosen.is_some() {
            return Err("arch is x86_64 or aarch64");
        }
        chosen = Some(value);
    }
    match chosen {
        None | Some("x86_64") => Ok("x86_64"),
        Some("aarch64") => Ok("aarch64"),
        Some(_) => Err("arch is x86_64 or aarch64"),
    }
}

/// `https://host:port` from the request, or nothing. The string is written into a shell script, so
/// it may only be a host and a port.
fn public_origin(https: bool, host: &str) -> Option<String> {
    let ok = !host.is_empty()
        && host.len() <= 253
        && host.chars().any(|c| c.is_ascii_alphanumeric())
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
    ok.then(|| format!("{}://{host}", if https { "https" } else { "http" }))
}

/// Downloads the binary, puts it on `~/.local/bin`, and runs unified with helloworld's task. The
/// label is the one a worker of this architecture already has, so the example needs nothing enrolled.
fn install_script(origin: &str, arch: &str) -> String {
    let label = if arch == "aarch64" { "arm" } else { "x86" };
    format!(
        r#"#!/bin/sh
set -eu
bindir=$HOME/.local/bin
mkdir -p "$bindir" "$HOME/bettertest"
curl -fsSL "{origin}/dl/bettertest-{arch}" -o "$bindir/bettertest"
chmod 755 "$bindir/bettertest"
cat > "$HOME/bettertest/hello.py" << 'PY'
from bettertest import Labels, Stage, task


class Label(Labels):
    {label} = "{label}"


class Hello(Stage):
    labels = (Label.{label},)

    @task()
    def world():
        print("hello, world")
PY
exec "$bindir/bettertest" unified "$HOME/bettertest/hello.py"
"#
    )
}

async fn install_sh(
    State(boss): State<Shared>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Failure> {
    let arch = download_arch(uri.query()).map_err(bad_request)?;
    let host = text_header(&headers, header::HOST);
    let origin = public_origin(boss.tls.is_some(), host);
    let origin = origin.ok_or_else(|| bad_request("need a plain host header"))?;
    let headers = [
        (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
        (header::CACHE_CONTROL, "no-cache"),
    ];
    Ok((headers, install_script(&origin, arch)).into_response())
}

async fn download_binary(AxumPath(name): AxumPath<String>) -> Result<Response, Failure> {
    let Some(path) = binary_file(&name) else {
        return Err(not_found("such binary"));
    };
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => not_found(format!("{name} in {DL_DIR}")),
            _ => internal(e),
        })?;
    let length = file.metadata().await.map_err(internal)?.len();
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CACHE_CONTROL, "no-cache".to_string()),
            (header::CONTENT_LENGTH, length.to_string()),
        ],
        Body::from_stream(ReaderStream::new(file)),
    )
        .into_response())
}

/// There is no landing page: `/` is the first pipeline's page. It is served here rather than
/// redirected to, which would cost a round trip, and names that pipeline as canonical for the app.
async fn home(headers: HeaderMap, State(boss): State<Shared>) -> Response {
    let (Some(first), Some(index)) = (
        boss.pipelines().into_iter().next(),
        EmbeddedWebAssets::get("index.html"),
    ) else {
        return (StatusCode::NOT_FOUND, "no pipelines").into_response();
    };
    // The canonical link is spliced in, so this cannot use index.html's precompressed copy.
    // Gzip it the same way: the document has to stay inside the first burst.
    document(&headers, with_canonical(&index.data, &first))
}

/// The HTML document. Gzipped when the browser asks and that is smaller.
fn document(headers: &HeaderMap, html: String) -> Response {
    let plain = html.into_bytes();
    let gzip = accepts_gzip(text_header(headers, header::ACCEPT_ENCODING))
        .then(|| gzip_smaller(&plain))
        .flatten();
    let common = [
        (header::CONTENT_TYPE, "text/html; charset=utf-8"),
        (header::CACHE_CONTROL, "no-cache"),
    ];
    let encoded = gzip.is_some().then_some([
        (header::VARY, "accept-encoding"),
        (header::CONTENT_ENCODING, "gzip"),
    ]);
    (common, encoded, gzip.unwrap_or(plain)).into_response()
}

/// Header `name` as text, or "" without one.
fn text_header(headers: &HeaderMap, name: header::HeaderName) -> &str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

/// Gzip of `plain` when it is actually smaller. None leaves the caller sending `plain`: the
/// document still answers, and a Vec encode does not fail short of memory.
fn gzip_smaller(plain: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(plain).ok()?;
    let gzip = enc.finish().ok()?;
    (gzip.len() < plain.len()).then_some(gzip)
}

fn with_canonical(index: &[u8], pipeline: &str) -> String {
    let link = format!(r#"<link rel="canonical" href="/pipeline/{pipeline}"></head>"#);
    String::from_utf8_lossy(index).replacen("</head>", &link, 1)
}

async fn index(headers: HeaderMap) -> Response {
    embedded::<EmbeddedWebAssets>(&headers, "index.html")
}

async fn get_embedded_asset(headers: HeaderMap, AxumPath(path): AxumPath<String>) -> Response {
    embedded::<EmbeddedWebAssets>(&headers, &path)
}

/// A file of the web ui, as the gzip copy build.rs made of it if the browser takes gzip.
fn embedded<Files: rust_embed::RustEmbed>(headers: &HeaderMap, path: &str) -> Response {
    let Some(file) = Files::get(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let gzip = Files::get(&format!("{path}.gz"));
    let vary = gzip
        .is_some()
        .then_some([(header::VARY, "accept-encoding")]);
    let accepted = text_header(headers, header::ACCEPT_ENCODING);
    // index.html names the hashed shell, so a browser has to recheck it, and so do wasm-bindgen's
    // snippets, whose names stay the same when their code changes. Everything else has a content
    // hash or, under assets/, a version in its name, and can be kept forever.
    let cache = match path == "index.html" || path.starts_with("snippets/") {
        true => "no-cache",
        false => "public, max-age=31536000, immutable",
    };
    let headers = [
        (header::CONTENT_TYPE, file.metadata.mimetype()),
        (header::CACHE_CONTROL, cache),
    ];
    let gzip = gzip.filter(|_| accepts_gzip(accepted));
    let encoded = gzip
        .is_some()
        .then_some([(header::CONTENT_ENCODING, "gzip")]);
    (headers, vary, encoded, gzip.map_or(file.data, |g| g.data)).into_response()
}

/// `gzip;q=0` refuses gzip. Names are matched case-insensitively, as the spec says.
fn accepts_gzip(header: &str) -> bool {
    header.split(',').any(|part| {
        let mut params = part.split(';').map(str::trim);
        let gzip = params
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("gzip"));
        gzip && params.all(|param| match param.split_once('=') {
            Some((name, q)) if name.trim().eq_ignore_ascii_case("q") => {
                q.trim().parse::<f64>().ok().is_none_or(|q| q > 0.0)
            }
            _ => true,
        })
    })
}

impl Boss {
    fn db(&self) -> MutexGuard<'_, Db> {
        lock(&self.db)
    }

    fn pipelines(&self) -> Vec<String> {
        lock(&self.pipedefs).keys().cloned().collect()
    }

    fn pipedef(&self, name: &str) -> Option<Pipedef> {
        lock(&self.pipedefs).get(name).cloned()
    }

    fn repo(&self, index: usize) -> Result<&Repo, Failure> {
        self.repos
            .get(index)
            .ok_or_else(|| internal("no such repo"))
    }

    /// Fetches repo number `index` and adds the pipelines its default branch has. A fetch that
    /// fails leaves the mirror as it was, which is still worth reading.
    async fn fetch(&self, index: usize) {
        let Some(repo) = self.repos.get(index) else {
            return;
        };
        if let Err(e) = repo.fetch().await {
            eprintln!("could not fetch {}: {e}", repo.public);
        }
        let found = match repo.pipedefs("HEAD").await {
            Ok(found) => found,
            Err(e) => {
                eprintln!("could not list the pipedefs of {}: {e}", repo.public);
                return;
            }
        };
        // The lock ends with this block, before anything below waits.
        let names: Vec<String> = {
            let mut pipedefs = lock(&self.pipedefs);
            for (name, path) in found {
                if !valid_pipeline_name(&name) {
                    eprintln!(
                        "{} has {path}: pipedef names are letters, digits, '-' and '_'",
                        repo.public
                    );
                    continue;
                }
                match pipedefs.get(&name) {
                    Some(Pipedef::Repo { repo: other, .. }) if *other == index => {}
                    Some(_) => eprintln!(
                        "{} has {path}, but pipeline {name} exists already",
                        repo.public
                    ),
                    None => {
                        pipedefs.insert(name, Pipedef::Repo { repo: index, path });
                    }
                }
            }
            let ours = |p: &Pipedef| matches!(p, Pipedef::Repo { repo, .. } if *repo == index);
            let ours = pipedefs.iter().filter(|(_, p)| ours(p));
            ours.map(|(name, _)| name.clone()).collect()
        };
        // What the pages show is the default branch as of this fetch.
        for name in names {
            if let Err((_, e)) = refresh_shown(self, &name, None).await {
                eprintln!("could not read pipeline {name}: {e}");
            }
        }
    }

    /// Tells the forge how a run of a pipeline in a repo is doing on its commit.
    async fn report(&self, run: &Run, state: &str, description: &str, link: Option<&str>) {
        let Some(Pipedef::Repo { repo, .. }) = self.pipedef(&run.pipeline) else {
            return;
        };
        let (Some(commit), Some(repo)) = (&run.commit, self.repos.get(repo)) else {
            return;
        };
        let reported = repo
            .report(
                &self.http,
                &commit.sha,
                &run.pipeline,
                state,
                description,
                link,
            )
            .await;
        if let Err(e) = reported {
            eprintln!("could not tell {} about run {}: {e}", repo.public, run.id);
        }
    }

    /// Writes to the db and broadcasts the resulting event under one lock. Paired with
    /// `subscribe`, this is what makes "snapshot, then live events" gapless.
    fn record<T>(
        &self,
        write: impl FnOnce(&mut Db) -> rusqlite::Result<(Event, T)>,
    ) -> rusqlite::Result<T> {
        let mut db = self.db();
        let (event, value) = write(&mut db)?;
        // Err only means nobody is watching right now.
        let _ = self.events.send(event);
        Ok(value)
    }

    /// Takes a snapshot and subscribes to events under the same lock `record` holds. Every event
    /// is either already in the snapshot or arrives on the receiver. Never both, never neither.
    fn subscribe<S>(
        &self,
        snapshot: impl FnOnce(&Db) -> rusqlite::Result<S>,
    ) -> rusqlite::Result<(S, broadcast::Receiver<Event>)> {
        let db = self.db();
        Ok((snapshot(&db)?, self.events.subscribe()))
    }

    // A db write failing mid-run is no reason to stop the run, so these log and carry on.

    fn set_task_state(&self, run: &Run, task: Uuid, state: TaskState) {
        let written = self.record(|db| {
            let at = now();
            db.set_task_state(task, state, at)?;
            let event = Event::TaskState {
                pipeline: run.pipeline.clone(),
                run: run.id,
                task,
                state,
                at,
            };
            Ok((event, ()))
        });
        if let Err(e) = written {
            eprintln!("could not record task {task} as {}: {e}", state.as_str());
        }
        if state.is_over() {
            self.task_ended(task);
        }
    }

    /// Deletes the logs and artifacts the settings keep no longer, as of `now`, a batch at a time
    /// so that running tasks can log in between. Returns how many tasks lost their log, and how
    /// many their artifacts.
    fn sweep(&self, now: u64) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        const BATCH: i64 = 100;
        const DAY: u64 = 24 * 60 * 60 * 1000;
        let settings = self.db().settings()?;
        let cutoff = |days: u32| now.saturating_sub(u64::from(days) * DAY);
        let (mut logs, mut artifacts) = (0, 0);
        if let Some(days) = settings.log_days {
            loop {
                match self.db().expire_logs(cutoff(days), now, BATCH)? {
                    0 => break,
                    n => logs += n,
                }
            }
        }
        if let Some(days) = settings.artifact_days {
            loop {
                let tasks = self.db().expiring_artifacts(cutoff(days), BATCH)?;
                if tasks.is_empty() {
                    break;
                }
                // Files first: should forgetting them fail, the next sweep deletes them again
                // rather than leaving files nothing refers to.
                for task in &tasks {
                    let dir = task_artifacts_dir(*task)?;
                    if let Err(e) = fs::remove_dir_all(&dir)
                        && e.kind() != ErrorKind::NotFound
                    {
                        return Err(format!("could not delete {}: {e}", dir.display()).into());
                    }
                }
                self.db().forget_artifacts(&tasks, now)?;
                artifacts += tasks.len();
            }
        }
        if logs + artifacts > 0 {
            println!("retention deleted the logs of {logs} tasks and the artifacts of {artifacts}");
        }
        Ok((logs, artifacts))
    }

    fn set_run_state(&self, run: &Run, state: RunState) {
        let written = self.record(|db| {
            db.set_run_state(run.id, state)?;
            let event = Event::RunFinished {
                pipeline: run.pipeline.clone(),
                run: run.id,
                state,
            };
            Ok((event, ()))
        });
        if let Err(e) = written {
            eprintln!("could not record run {} as {}: {e}", run.id, state.as_str());
        }
    }

    fn log(&self, task: Uuid, stream: LogStream, line: String) {
        self.log_lines(task, vec![LogLine { stream, line }]);
    }

    /// Like `record`: what is written is broadcast under the same lock.
    fn log_lines(&self, task: Uuid, lines: Vec<LogLine>) {
        if lines.is_empty() {
            return;
        }
        let mut db = self.db();
        if let Err(e) = db.append_log(task, &lines) {
            eprintln!("could not record log lines of task {task}: {e}");
            return;
        }
        for line in lines {
            // Err only means nobody is watching right now.
            let _ = self.events.send(Event::Log { task, line });
        }
    }
}

#[cfg(test)]
#[path = "../../test/boss.rs"]
mod tests;
