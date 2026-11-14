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
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{self, KeepAlive, Sse},
    },
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
    // Woken whenever a worker changes, which is when a task waiting for a busy worker may get one.
    freed: Notify,
    http: reqwest::Client,
    // The certificate the boss presents, when it speaks HTTPS.
    tls: Option<tls::Tls>,
    // Where the web UI and API listen. Workers call back here: they take the IP from the
    // start-task connection and this port from that same form.
    port: u16,
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

    let mut db = db::open()?;
    let abandoned = db.fail_abandoned_runs(now())?;
    db.heartbeat("boss", now())?;
    if abandoned > 0 {
        println!("marked {abandoned} runs from a previous boss as failed");
    }
    let enrolled = db.workers()?;

    // Big enough that only a badly stuck browser falls behind. One that does gets a fresh snapshot.
    let (events, _) = broadcast::channel(4096);
    let boss = Arc::new(Boss {
        pipedefs: Mutex::new(names),
        repos,
        going: Mutex::new(HashMap::new()),
        db: Mutex::new(db),
        workers: Mutex::new(BTreeMap::new()),
        task_tokens: Mutex::new(HashMap::new()),
        ceremonies: Mutex::new(HashMap::new()),
        events,
        freed: Notify::new(),
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
    spawn(heartbeat(boss.clone()));
    for index in 0..boss.repos.len() {
        boss.fetch(index).await;
    }
    if boss.pipelines().is_empty() {
        println!("no pipedefs yet. add some and restart the boss");
    }

    // The router takes the boss along; this keeps hold of its certificate for serving.
    let serving = boss.clone();
    let router = Router::new()
        .route("/api/pipelines", get(get_pipelines))
        .route("/api/pipeline/{name}", get(get_pipeline))
        .route("/api/pipeline/{name}/events", get(pipeline_events))
        .route("/api/start-run/{name}", post(start_run))
        .route("/api/push-hook", post(push_hook))
        .route("/api/cancel-run/{run}", post(cancel_run))
        .route("/api/rerun/{*rerun}", post(rerun))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/me", get(get_me))
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
            println!(
                "certificate for {}, expires {}",
                tls.names.join(", "),
                tls.expires
            );
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

fn not_found(what: impl std::fmt::Display) -> Failure {
    (StatusCode::NOT_FOUND, format!("no {what}"))
}

fn parse_id(id: &str) -> Result<Uuid, Failure> {
    Uuid::parse_str(id).map_err(|_| (StatusCode::BAD_REQUEST, format!("{id:?} is not a uuid")))
}

/// Writes the boss's heartbeat every 15 seconds, so that after a crash or a power cut the next boss
/// knows when this one stopped.
async fn heartbeat(boss: Shared) {
    let mut beat = tokio::time::interval(HEARTBEAT);
    loop {
        beat.tick().await;
        if let Err(e) = boss.db().heartbeat("boss", now()) {
            eprintln!("could not record the boss's heartbeat: {e}");
        }
    }
}

const HEARTBEAT: Duration = Duration::from_secs(15);

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

#[derive(Serialize)]
struct Swept {
    // How many tasks lost their log and their artifacts to the new settings.
    logs: usize,
    artifacts: usize,
}

/// Saves the settings and applies them at once, so what they no longer keep is gone by the time
/// this returns.
async fn put_settings(
    State(boss): State<Shared>,
    Json(settings): Json<Settings>,
) -> Result<Json<Swept>, Failure> {
    for days in [settings.artifact_days, settings.log_days]
        .into_iter()
        .flatten()
    {
        if !(1..=36500).contains(&days) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("keep things 1 to 36500 days, or forever, not {days} days"),
            ));
        }
    }
    boss.db().set_settings(&settings).map_err(internal)?;
    let (logs, artifacts) = boss.sweep(now()).map_err(internal)?;
    Ok(Json(Swept { logs, artifacts }))
}

/// An SSE stream that starts with a snapshot, then sends every event `wanted` keeps. A client too
/// slow to keep up gets a fresh snapshot rather than a gap. `snapshot` returning None means there
/// is no such thing to watch.
fn live<S: Serialize + Send + 'static>(
    boss: Shared,
    snapshot: impl Fn(&Db) -> rusqlite::Result<Option<S>> + Send + 'static,
    wanted: impl Fn(&S, &Event) -> bool + Send + 'static,
) -> Result<Response, Failure> {
    let (current, mut events) = boss.subscribe(&snapshot).map_err(internal)?;
    let Some(mut current) = current else {
        return Err((StatusCode::NOT_FOUND, "not found".into()));
    };

    let (tx, rx) = mpsc::channel::<sse::Event>(64);
    spawn(async move {
        loop {
            let message = sse_message(&Live::Snapshot(&current));
            if tx.send(message).await.is_err() {
                return;
            }
            loop {
                let received = tokio::select! {
                    _ = tx.closed() => return,
                    received = events.recv() => received,
                };
                match received {
                    Ok(event) if wanted(&current, &event) => {
                        let message = sse_message(&Live::<&S>::Event(event));
                        if tx.send(message).await.is_err() {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => break,
                    Err(broadcast::error::RecvError::Closed) => return,
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

    let stream = ReceiverStream::new(rx).map(Ok::<_, Infallible>);
    // A named event rather than the usual comment, which browsers keep from the page: hearing
    // nothing, not even this, is how the page tells a lost boss from a quiet one.
    let ping = KeepAlive::new()
        .interval(PING)
        .event(sse::Event::default().event("ping"));
    Ok(Sse::new(stream).keep_alive(ping).into_response())
}

/// How often a live stream says it is still there when there is nothing else to say.
const PING: Duration = Duration::from_secs(15);

fn sse_message(data: &impl Serialize) -> sse::Event {
    // Serializing our own plain structs cannot fail. If it somehow did, an empty event is harmless.
    sse::Event::default().data(serde_json::to_string(data).unwrap_or_default())
}

/// Pipeline names end up in URLs.
fn valid_pipeline_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// There is no landing page: `/` is the first pipeline's page. It is served here rather than
/// redirected to, which would cost a round trip, and names that pipeline as canonical for the app.
async fn home(State(boss): State<Shared>) -> Response {
    let (Some(first), Some(index)) = (
        boss.pipelines().into_iter().next(),
        EmbeddedWebAssets::get("index.html"),
    ) else {
        return (StatusCode::NOT_FOUND, "no pipelines").into_response();
    };
    // Not compressed, unlike index.html: it fits in the first burst all the same.
    let headers = [
        (header::CONTENT_TYPE, "text/html; charset=utf-8"),
        (header::CACHE_CONTROL, "no-cache"),
    ];
    (headers, with_canonical(&index.data, &first)).into_response()
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
    let accepted = headers.get(header::ACCEPT_ENCODING);
    let accepted = accepted.and_then(|v| v.to_str().ok()).unwrap_or_default();
    // index.html names the hashed shell, so a browser has to recheck it. Everything else has a
    // content hash or, under assets/, a version in its name, and can be kept forever.
    let cache = match path {
        "index.html" => "no-cache",
        _ => "public, max-age=31536000, immutable",
    };
    let headers = [
        (header::CONTENT_TYPE, file.metadata.mimetype()),
        (header::CACHE_CONTROL, cache),
    ];
    match gzip.filter(|_| accepts_gzip(accepted)) {
        Some(gzip) => (
            headers,
            vary,
            [(header::CONTENT_ENCODING, "gzip")],
            gzip.data,
        )
            .into_response(),
        None => (headers, vary, file.data).into_response(),
    }
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
    }

    /// Tells the forge how a run of a pipeline in a repo is doing on its commit.
    async fn report(&self, run: &Run, state: &str, description: &str, link: Option<&str>) {
        let Some(commit) = &run.commit else {
            return;
        };
        let Some(Pipedef::Repo { repo, .. }) = self.pipedef(&run.pipeline) else {
            return;
        };
        let Some(repo) = self.repos.get(repo) else {
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
    fn sweep(&self, now: u64) -> Result<(usize, usize), String> {
        const BATCH: i64 = 100;
        const DAY: u64 = 24 * 60 * 60 * 1000;
        let settings = self.db().settings().map_err(|e| e.to_string())?;
        let cutoff = |days: u32| now.saturating_sub(u64::from(days) * DAY);
        let (mut logs, mut artifacts) = (0, 0);
        if let Some(days) = settings.log_days {
            loop {
                let expired = self.db().expire_logs(cutoff(days), now, BATCH);
                match expired.map_err(|e| e.to_string())? {
                    0 => break,
                    n => logs += n,
                }
            }
        }
        if let Some(days) = settings.artifact_days {
            loop {
                let expiring = self.db().expiring_artifacts(cutoff(days), BATCH);
                let tasks = expiring.map_err(|e| e.to_string())?;
                if tasks.is_empty() {
                    break;
                }
                // Files first: should forgetting them fail, the next sweep deletes them again
                // rather than leaving files nothing refers to.
                for task in &tasks {
                    let dir = task_artifacts_dir(*task).map_err(|e| e.to_string())?;
                    match fs::remove_dir_all(&dir) {
                        Err(e) if e.kind() != ErrorKind::NotFound => {
                            return Err(format!("could not delete {}: {e}", dir.display()));
                        }
                        _ => {}
                    }
                }
                let forgotten = self.db().forget_artifacts(&tasks, now);
                forgotten.map_err(|e| e.to_string())?;
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
