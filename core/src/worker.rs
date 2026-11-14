// The worker runs tasks and the sandboxes they run in. It keeps nothing between restarts: the boss
// (or dispatch) decides what runs where, and starts and stops sandboxes explicitly, so they can be
// shared by several tasks or started early to have them warm.

use crate::{
    boss::{self, Failure, internal},
    client, lock, pipedef, vm, workdir,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, FromRef, Multipart, Path, State, multipart::MultipartError},
    http::{StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
    serve::ListenerExt,
};
use bettertest_shared::{Arch, WorkerInfo, valid_artifact_name};
use serde::{Deserialize, Serialize};
use std::{
    convert::Infallible,
    error::Error,
    fs, io,
    net::SocketAddr,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::{Child, Command},
    spawn,
    sync::{Semaphore, mpsc},
};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

pub const LISTEN_PORT: u16 = 7358;

// Every container we start carries this label, so ones a crashed worker left behind can be found.
const CONTAINER_LABEL: &str = "bettertest.worker";

#[derive(Serialize, Deserialize)]
pub struct StartContainer {
    // A docker image name, or the URL of a `docker save` tarball. The pipedef names that source.
    pub image: String,
}

#[derive(Serialize, Deserialize)]
pub struct StartVm {
    // URL of a qcow2 disk image, the image's own. The worker downloads it.
    pub image: String,
    // None means this worker's own.
    pub arch: Option<Arch>,
}

#[derive(Serialize, Deserialize)]
pub struct Started {
    pub id: String,
}

/// The fields of a `start-task` form.
struct StartTask {
    pipedef: String,
    stage: String,
    task: String,
    // "container:<id>" or "vm:<id>". Absent for a task without a sandbox.
    target: Option<String>,
    // The pipeline and run the task belongs to. Absent when dispatch runs it outside any run.
    pipeline: Option<String>,
    run: Option<String>,
    // The commit the run tests, when its pipeline lives in a git repo. The task runs in a checkout
    // of it.
    commit: Option<String>,
    // When the boss speaks HTTPS: the name its certificate is for.
    boss_name: Option<String>,
    // What to show the boss when fetching the commit and artifacts, so a locked-down boss lets the
    // task read what its run needs.
    boss_token: Option<String>,
    // The port the boss listens on, so this worker can call it at the IP of the start-task
    // connection. Absent for dispatch (no boss) and then unused.
    boss_port: Option<u16>,
}

/// A task's files on this worker: `src/` is where it runs, holding the commit its run tests,
/// `inputs/` holds the artifacts it fetched from the boss, and `outputs/` is where artifact() puts
/// what it publishes. The caller fetches those before it hangs
/// up. Removed when dropped, so a worker keeps nothing once the task is over.
struct TaskFiles {
    id: Uuid,
    dir: PathBuf,
}

impl TaskFiles {
    fn create() -> io::Result<Self> {
        let id = Uuid::new_v4();
        let dir = workdir::dir("tasks")?.join(id.to_string());
        workdir::shared_dir(&dir)?;
        workdir::shared_dir(&dir.join("inputs"))?;
        workdir::shared_dir(&dir.join("outputs"))?;
        workdir::shared_dir(&dir.join("src"))?;
        Ok(Self { id, dir })
    }

    fn src(&self) -> PathBuf {
        self.dir.join("src")
    }

    fn inputs(&self) -> PathBuf {
        self.dir.join("inputs")
    }

    fn outputs(&self) -> PathBuf {
        self.dir.join("outputs")
    }

    /// The names of the files the task published, sorted.
    fn published(&self) -> io::Result<Vec<String>> {
        let mut names = vec![];
        for entry in fs::read_dir(self.outputs())? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // artifact() only ever writes files with valid names. Anything else is not ours to
            // hand out.
            if entry.file_type()?.is_file() && valid_artifact_name(&name) {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }
}

impl Drop for TaskFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// What the worker's handlers share: its VMs, and a permit for each task it may run at once.
#[derive(Clone)]
struct Shared {
    vms: Arc<vm::Vms>,
    slots: Arc<Semaphore>,
    /// Pythons started before there was a task for them, at most one a slot (see ready_python).
    ready: Arc<Mutex<Vec<Child>>>,
    capacity: usize,
}

impl FromRef<Shared> for Arc<vm::Vms> {
    fn from_ref(shared: &Shared) -> Self {
        shared.vms.clone()
    }
}

/// `labels` are what tasks can pick this worker by, besides its architecture's. `capacity` is how
/// many tasks it runs at once. `port` is where it listens (`--worker-port`, 7358 unless told
/// otherwise).
pub async fn entry(labels: Vec<String>, capacity: u32, port: u16) -> Result<(), Box<dyn Error>> {
    // Labels go into pipedefs as enum values and onto the workers page: keep them plain names.
    if let Some(bad) = labels.iter().find(|l| !valid_artifact_name(l)) {
        return Err(format!("label {bad:?}: labels are letters, digits, '.', '-' and '_'").into());
    }
    let native = vm::NATIVE.label().to_string();
    let mut labels: Vec<String> = labels.into_iter().filter(|l| *l != native).collect();
    labels.insert(0, native);
    // Anything left over is from a worker process that died. Nothing will ever stop it otherwise.
    // Failing to clean up is worth a warning, not worth refusing to start.
    if let Err(e) = remove_leftover_containers().await {
        eprintln!("could not remove leftover containers: {e}");
    }
    if let Err(e) = vm::remove_leftovers() {
        eprintln!("could not remove leftover vms: {e}");
    }
    for name in ["tasks", "containers"] {
        if let Err(e) = remove_leftovers(name) {
            eprintln!("could not remove leftover {name}: {e}");
        }
    }

    let shared = Shared {
        vms: Arc::new(vm::Vms::new(port)),
        slots: Arc::new(Semaphore::new(capacity as usize)),
        ready: Arc::default(),
        capacity: capacity as usize,
    };
    let router = router(shared, labels, capacity);

    println!("worker listening on http://[::1]:{port}");
    let listener = TcpListener::bind(("::", port)).await?;
    // start-task learns the boss's address from the connection it arrives on.
    let service = router.into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener.tap_io(crate::no_delay), service).await?;
    Ok(())
}

fn router(shared: Shared, labels: Vec<String>, capacity: u32) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/events", get(move || events(labels.clone(), capacity)))
        .route("/start-container", post(start_container))
        .route("/container/{id}", delete(delete_container))
        .route("/start-vm", post(start_vm))
        .route("/vm/{id}", delete(delete_vm))
        .route("/vm/{id}/reboot", post(reboot_vm))
        .route("/vm/{id}/ready", post(vm_ready))
        .route("/vm/{id}/{file}", get(vm_cloud_init))
        .route("/start-task", post(start_task))
        .route("/output/{id}/{name}", get(get_output))
        .with_state(shared)
}

async fn health() -> &'static str {
    "ok"
}

/// For the boss: what this worker is, then nothing but keep-alives for as long as it is up. The boss
/// counts the worker as connected while this stream lasts.
async fn events(labels: Vec<String>, capacity: u32) -> impl IntoResponse {
    let info = WorkerInfo {
        // Only for people to read, so a machine without one is fine.
        hostname: fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|name| name.trim().to_string())
            .unwrap_or_default(),
        arch: vm::NATIVE,
        version: env!("CARGO_PKG_VERSION").to_string(),
        labels,
        capacity,
    };
    let hello = Event::default()
        .event("worker")
        .data(serde_json::to_string(&info).unwrap_or_default());
    let stream = tokio_stream::once(hello).chain(tokio_stream::pending());
    Sse::new(stream.map(Ok::<_, Infallible>)).keep_alive(KeepAlive::default())
}

async fn start_container(Json(req): Json<StartContainer>) -> Result<Json<Started>, Failure> {
    let image = if is_url(&req.image) {
        load_image(&req.image).await?
    } else {
        req.image
    };
    let id = Uuid::new_v4().to_string();
    let channel = channel(&id).map_err(internal)?;
    workdir::shared_dir(&channel).map_err(internal)?;
    let commands = channel.join("commands");
    output(Command::new("mkfifo").args(["-m", "666"]).arg(&commands))
        .await
        .map_err(internal)?;
    let mount = format!("{}:/.bettertest:z", channel.display());
    let mut run = Command::new("docker");
    run.args(["run", "--detach", "--init", "--label", CONTAINER_LABEL])
        .args(["--name", &id, "--volume", &mount])
        .args(["--entrypoint", "/bin/sh", &image, "-c", SERVE_COMMANDS]);
    if let Err(e) = output(&mut run).await {
        let _ = fs::remove_dir_all(&channel);
        return Err(internal(e));
    }
    Ok(Json(Started { id }))
}

/// The container's main process, from start to delete: it runs each command run() names in
/// `commands`, in the background, so tasks of a parallel stage run at the same time, without a
/// docker exec each (60 ms). The outer subshell exits at once, so tini reaps the command.
/// Opened for writing too, the FIFO never reads as ended: closing it to reopen would throw away a
/// name written in between.
const SERVE_COMMANDS: &str = r#"while read -r name; do
    d=/.bettertest/$name
    ( { sh "$d/command" < /dev/null > "$d/out" 2> "$d/err"; echo $? > "$d/exit"; } & )
done 0<> /.bettertest/commands"#;

/// The directory on the worker a container shares with the tasks that run commands in it.
fn channel(id: &str) -> Result<PathBuf, String> {
    let containers = workdir::dir("containers").map_err(|e| e.to_string())?;
    Ok(containers.join(id))
}

async fn delete_container(Path(id): Path<Uuid>) -> Result<(), Failure> {
    let id = id.to_string();
    output(Command::new("docker").args(["rm", "--force", "--volumes", &id]))
        .await
        .map_err(internal)?;
    fs::remove_dir_all(channel(&id).map_err(internal)?).map_err(internal)?;
    Ok(())
}

async fn remove_leftover_containers() -> Result<(), String> {
    let label = format!("label={CONTAINER_LABEL}");
    let ids =
        output(Command::new("docker").args(["ps", "--all", "--quiet", "--filter", &label])).await?;
    let ids: Vec<&str> = ids.split_whitespace().collect();
    if !ids.is_empty() {
        println!("removing {} leftover containers", ids.len());
        output(
            Command::new("docker")
                .args(["rm", "--force", "--volumes"])
                .args(&ids),
        )
        .await?;
    }
    Ok(())
}

/// Runs a command to completion. Ok is its stdout; Err explains what went wrong.
pub async fn output(command: &mut Command) -> Result<String, String> {
    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    let output = command
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("could not run {program}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn is_url(image: &str) -> bool {
    image.starts_with("http://") || image.starts_with("https://")
}

/// FNV-1a. A stable name for a cached download, derived from its URL. The pipedef names the source;
/// this cache is the worker's, and a later change at the same URL is not fetched again.
pub fn cache_key(url: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in url.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Makes the `docker save` tarball at `url` available as a local image, downloading it only the
/// first time. Returns the local image name.
async fn load_image(url: &str) -> Result<String, Failure> {
    let name = format!("bettertest-artifact:{}", cache_key(url));
    if output(Command::new("docker").args(["image", "inspect", &name]))
        .await
        .is_ok()
    {
        return Ok(name);
    }

    let mut response = download(url).await.map_err(internal)?;
    let mut load = Command::new("docker")
        .arg("load")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| internal(format!("could not run docker: {e}")))?;
    let Some(mut stdin) = load.stdin.take() else {
        return Err(internal("docker load has no stdin"));
    };
    while let Some(chunk) = response.chunk().await.map_err(internal)? {
        stdin.write_all(&chunk).await.map_err(internal)?;
    }
    drop(stdin);
    let finished = load.wait_with_output().await.map_err(internal)?;
    if !finished.status.success() {
        return Err(internal(String::from_utf8_lossy(&finished.stderr)));
    }

    // docker load prints "Loaded image: <name>", or "Loaded image ID: <id>" for untagged images.
    let stdout = String::from_utf8_lossy(&finished.stdout);
    let Some(loaded) = stdout.lines().find_map(|line| {
        line.strip_prefix("Loaded image: ")
            .or_else(|| line.strip_prefix("Loaded image ID: "))
    }) else {
        return Err(internal(format!("unexpected docker load output: {stdout}")));
    };
    output(Command::new("docker").args(["tag", loaded.trim(), &name]))
        .await
        .map_err(internal)?;
    Ok(name)
}

/// Unpacks the tarball at `url` into `dir`, fetched with the task's `token` when it has one.
async fn check_out(
    http: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    dir: &std::path::Path,
) -> Result<(), String> {
    let mut request = http.get(url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let mut response = request
        .send()
        .await
        .map_err(|e| format!("could not download {url}: {}", client::why(&e)))?;
    if !response.status().is_success() {
        let status = response.status();
        let why = response.text().await.unwrap_or_default();
        return Err(format!("could not download {url}: {status} {why}"));
    }
    let mut tar = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not run tar: {e}"))?;
    let Some(mut stdin) = tar.stdin.take() else {
        return Err("tar has no stdin".into());
    };
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("lost connection to {url}: {}", client::why(&e)))?
    {
        stdin
            .write_all(&chunk)
            .await
            .map_err(|e| format!("tar stopped reading: {e}"))?;
    }
    drop(stdin);
    let finished = tar.wait_with_output().await.map_err(|e| e.to_string())?;
    if !finished.status.success() {
        return Err(format!(
            "could not unpack {url}: {}",
            String::from_utf8_lossy(&finished.stderr)
        ));
    }
    Ok(())
}

pub async fn download(url: &str) -> Result<reqwest::Response, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("could not download {url}: {}", client::why(&e)))?;
    if !response.status().is_success() {
        return Err(format!("could not download {url}: {}", response.status()));
    }
    Ok(response)
}

async fn start_vm(
    State(vms): State<Arc<vm::Vms>>,
    Json(req): Json<StartVm>,
) -> Result<Json<Started>, Failure> {
    let id = vms.start(&req.image, req.arch).await.map_err(internal)?;
    Ok(Json(Started { id }))
}

async fn reboot_vm(State(vms): State<Arc<vm::Vms>>, Path(id): Path<String>) -> Result<(), Failure> {
    vms.reboot(&id).await.map_err(|e| {
        // The task log is the response body. Say it here too: a journal that only has the
        // request line hides a boot that never called back.
        eprintln!("reboot {id} failed: {e}");
        internal(e)
    })
}

async fn delete_vm(State(vms): State<Arc<vm::Vms>>, Path(id): Path<String>) -> Result<(), Failure> {
    vms.delete(&id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e))
}

// The two endpoints below are for the vm itself, not for the boss. It reaches the worker at
// 10.0.2.2, qemu's address for the host.

async fn vm_cloud_init(
    State(vms): State<Arc<vm::Vms>>,
    Path((id, file)): Path<(String, String)>,
) -> Result<String, StatusCode> {
    vms.cloud_init(&id, &file).ok_or(StatusCode::NOT_FOUND)
}

async fn vm_ready(State(vms): State<Arc<vm::Vms>>, Path(id): Path<String>) -> StatusCode {
    if vms.ready(&id) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// Empties `/var/tmp/bettertest/<name>`.
fn remove_leftovers(name: &str) -> io::Result<()> {
    for entry in fs::read_dir(workdir::dir(name)?)? {
        fs::remove_dir_all(entry?.path())?;
    }
    Ok(())
}

async fn read_start_task(mut form: Multipart) -> Result<StartTask, Failure> {
    let bad = |e: MultipartError| (StatusCode::BAD_REQUEST, e.body_text());
    let (mut pipedef, mut stage, mut task, mut target) = (None, None, None, None);
    let (mut pipeline, mut run, mut commit) = (None, None, None);
    let (mut boss_token, mut boss_name, mut boss_port) = (None, None, None);
    while let Some(field) = form.next_field().await.map_err(bad)? {
        let name = field.name().unwrap_or_default().to_string();
        let value = Some(field.text().await.map_err(bad)?);
        match name.as_str() {
            "pipedef" => pipedef = value,
            "stage" => stage = value,
            "task" => task = value,
            "target" => target = value,
            "pipeline" => pipeline = value,
            "run" => run = value,
            "commit" => commit = value,
            "boss_token" => boss_token = value,
            "boss_name" => boss_name = value,
            "boss_port" => {
                let text = value.as_deref().unwrap_or("");
                match text.parse::<u16>() {
                    Ok(port) if port != 0 => boss_port = Some(port),
                    _ => {
                        return Err((StatusCode::BAD_REQUEST, format!("bad boss_port {text:?}")));
                    }
                }
            }
            _ => return Err((StatusCode::BAD_REQUEST, format!("unknown field {name:?}"))),
        }
    }
    let missing = |field: &str| (StatusCode::BAD_REQUEST, format!("{field} is missing"));
    Ok(StartTask {
        pipedef: pipedef.ok_or_else(|| missing("pipedef"))?,
        stage: stage.ok_or_else(|| missing("stage"))?,
        task: task.ok_or_else(|| missing("task"))?,
        target,
        pipeline,
        run,
        commit,
        boss_token,
        boss_name,
        boss_port,
    })
}

/// Streams the task body's output as SSE: `stdout` and `stderr` lines, then an `artifact` event
/// for each file it published (`<id>/<name>`, for `GET /output/<id>/<name>`), then `exit`. The
/// files stay until the caller hangs up, so it fetches them as the events arrive.
async fn start_task(
    State(shared): State<Shared>,
    ConnectInfo(caller): ConnectInfo<SocketAddr>,
    form: Multipart,
) -> Result<impl IntoResponse, Failure> {
    // Held while the task process is running. Released when that process has exited, before the
    // exit event: the boss starts the next task as soon as it sees that event, and the connection
    // stays open until the boss has read it. Holding the permit until hangup makes the next
    // start-task a 409 on a worker that takes one task at a time.
    let Ok(slot) = shared.slots.clone().try_acquire_owned() else {
        return Err((
            StatusCode::CONFLICT,
            "this worker is busy: it runs as many tasks at once as it was started to (worker --tasks)"
                .into(),
        ));
    };
    let vms = shared.vms;
    let req = read_start_task(form).await?;
    let files = TaskFiles::create().map_err(internal)?;

    let target = python_target(&vms, req.target.as_deref())?;
    // A task of a run talks to the boss, which is whoever just called: IP from this connection,
    // listen port from the form (TCP's source port is ephemeral).
    let address = SocketAddr::new(
        caller.ip().to_canonical(),
        req.boss_port.unwrap_or(boss::LISTEN_PORT),
    );
    let boss = boss_url(&req, address)?;
    if let (Some(_), Some(boss), Some(run)) = (&req.commit, &boss, &req.run) {
        let mut http = reqwest::Client::builder();
        if let Some(name) = &req.boss_name {
            http = http.resolve(name, address);
        }
        let http = http.build().map_err(internal)?;
        let url = format!("{boss}/api/run/{run}/source.tar");
        check_out(&http, &url, req.boss_token.as_deref(), &files.src())
            .await
            .map_err(internal)?;
    }

    let ready = lock(&shared.ready).pop();
    let mut child = match ready {
        Some(child) => child,
        None => ready_python().await.map_err(internal)?,
    };
    let (ready, capacity) = (shared.ready.clone(), shared.capacity);
    spawn(async move {
        // Failing, it fails the next task too, which says why.
        if let Ok(child) = ready_python().await {
            let mut ready = lock(&ready);
            if ready.len() < capacity {
                ready.push(child);
            }
        }
    });
    let group = child.id().map(ProcessGroup);
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(internal("python3 is missing a pipe"));
    };
    let task_files = serde_json::json!({
        "outputs": files.outputs(),
        "tests": files.dir.join("tests"),
        "inputs": files.inputs(),
        "boss": boss,
        "boss_token": req.boss_token,
        // Where the boss is, when its url names a host DNS may not know (HTTPS).
        "boss_address": req.boss_name.as_ref().map(|_| address.to_string()),
        "pipeline": req.pipeline,
        "run": req.run,
    });
    let task = Some((req.stage.as_str(), req.task.as_str()));
    let src = files.src();
    let payload = pipedef::glue_stdin(&req.pipedef, target, task_files, task, Some(&src), None);

    let (tx, rx) = mpsc::channel::<Event>(64);
    spawn(async move {
        let _group = group;
        let slot = slot;
        // A failed write means python died early. Its stderr and exit code will say why.
        let _ = stdin.write_all(&payload).await;
        drop(stdin);
        let finished = async {
            tokio::try_join!(
                pump(stdout, tx.clone(), "stdout"),
                pump(stderr, tx.clone(), "stderr"),
            )?;
            Ok::<_, ()>(child.wait().await)
        };
        // Returning early drops `child`, which kills the task, and `slot`.
        let status = tokio::select! {
            _ = tx.closed() => return, // The caller hung up.
            status = finished => status,
        };
        // The task is no longer running. Free the slot before the caller hears that, so it can
        // start another task while it is still reading this stream.
        drop(slot);
        // No exit code means a signal killed it.
        let code = match status {
            Ok(Ok(status)) => status.code().unwrap_or(-1),
            Ok(Err(_)) => -1,
            Err(()) => return, // The caller hung up mid-line.
        };
        let published = match files.published() {
            Ok(published) => published,
            Err(e) => {
                let line = format!("could not list the task's artifacts: {e}");
                let _ = tx.send(Event::default().event("stderr").data(line)).await;
                vec![]
            }
        };
        for name in published {
            let artifact = format!("{}/{name}", files.id);
            if tx
                .send(Event::default().event("artifact").data(artifact))
                .await
                .is_err()
            {
                return;
            }
        }
        // What the task said of its tests with tests(), as one line of json.
        if let Ok(tests) = fs::read_to_string(files.dir.join("tests")) {
            let _ = tx.send(Event::default().event("tests").data(tests)).await;
        }
        let _ = tx
            .send(Event::default().event("exit").data(code.to_string()))
            .await;
        // Dropping `files` removes them, so wait until the caller has what it wants.
        tx.closed().await;
    });

    let stream = ReceiverStream::new(rx).map(Ok::<_, Infallible>);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// A python that has the library and waits for the rest of glue.py's input, to start a task. Most of
/// starting a task is starting python and the library.
async fn ready_python() -> Result<Child, String> {
    // python -u: unbuffered, so lines show up as the task prints them, not in bursts.
    let mut child = Command::new("python3")
        .args(["-u", "-c", pipedef::GLUE, "start"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // If the caller hangs up, the stream below is dropped, and with it the task.
        .kill_on_drop(true)
        // A group of its own, so whatever python starts can be killed with it.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not start python3: {e}"))?;
    let stdin = child.stdin.as_mut().ok_or("python3 is missing a pipe")?;
    // Fits in the pipe, so this does not wait for python.
    let library = pipedef::library_line();
    stdin.write_all(&library).await.map_err(|e| e.to_string())?;
    Ok(child)
}

/// Where run() in python sends commands: the task's sandbox, as `start-task` names it
/// (`container:<id>`, `vm:<id>`), or none.
fn python_target(vms: &vm::Vms, target: Option<&str>) -> Result<serde_json::Value, Failure> {
    Ok(match target.map(|t| t.split_once(':')) {
        None => serde_json::Value::Null,
        Some(Some(("container", id))) => {
            let commands = channel(id).map_err(internal)?.join("commands");
            serde_json::json!({"kind": "container", "id": id, "commands": commands})
        }
        Some(Some(("vm", id))) => {
            let (port, key) = vms
                .ssh(id)
                .ok_or((StatusCode::NOT_FOUND, format!("no vm {id}")))?;
            let reboot = format!("http://127.0.0.1:{}/vm/{id}/reboot", vms.port());
            serde_json::json!({"kind": "vm", "port": port, "key": key, "reboot": reboot})
        }
        Some(_) => return Err((StatusCode::BAD_REQUEST, "bad target".into())),
    })
}

/// The boss a task of a run fetches its commit and artifacts from, which called from `address`.
/// None for a task of no run: dispatch is not a boss.
fn boss_url(req: &StartTask, address: SocketAddr) -> Result<Option<String>, Failure> {
    Ok(match (&req.pipeline, &req.run, &req.boss_name) {
        (Some(_), Some(run), None) if Uuid::parse_str(run).is_ok() => {
            Some(format!("http://{address}"))
        }
        // Over HTTPS, the certificate is for a name the boss may have no DNS record for. The task
        // connects to the address the boss called from, and checks the certificate against the name.
        (Some(_), Some(run), Some(name)) if Uuid::parse_str(run).is_ok() && valid_host(name) => {
            Some(format!("https://{name}:{}", address.port()))
        }
        (None, None, None) => None,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "bad pipeline, run or boss name".into(),
            ));
        }
    })
}

/// Every process a task started: python, and whatever it ran. Dropping this kills them all, so a
/// task that times out or is hung up on leaves nothing behind, not even a subprocess it started.
struct ProcessGroup(u32);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // std cannot signal a process group. The shell's builtin kill can, and unlike kill(1) the
        // shell is on every machine. Failing means nothing was left to kill.
        let _ = Command::new("sh")
            .args(["-c", &format!("kill -KILL -{}", self.0)])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// A host name, as a certificate names it: nothing that could turn a url into another one.
fn valid_host(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
}

async fn get_output(Path((id, name)): Path<(String, String)>) -> Result<Response, Failure> {
    let (Ok(id), true) = (Uuid::parse_str(&id), valid_artifact_name(&name)) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "bad task id or artifact name".into(),
        ));
    };
    let path = workdir::root()
        .map_err(internal)?
        .join("tasks")
        .join(id.to_string())
        .join("outputs")
        .join(&name);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{}: {e}", path.display())))?;
    let length = file.metadata().await.map_err(internal)?.len();
    let headers = [(header::CONTENT_LENGTH, length.to_string())];
    Ok((headers, Body::from_stream(ReaderStream::new(file))).into_response())
}

/// Sends each line of `reader` as an SSE event named `event`. Err means the receiver hung up.
async fn pump(
    reader: impl AsyncRead + Unpin,
    tx: mpsc::Sender<Event>,
    event: &str,
) -> Result<(), ()> {
    // Split raw bytes, not text: a task can print anything, including invalid UTF-8.
    let mut lines = BufReader::new(reader).split(b'\n');
    while let Ok(Some(bytes)) = lines.next_segment().await {
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_suffix('\r').unwrap_or(&text);
        // SSE cannot carry a carriage return. Progress bars use them to redraw a line, so treat
        // each redraw as its own line.
        for line in text.split('\r') {
            tx.send(Event::default().event(event).data(line))
                .await
                .map_err(|_| ())?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../test/worker.rs"]
mod tests;
