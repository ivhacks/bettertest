use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use bettertest_common::*;
use rust_embed::Embed;
use serde::Serialize;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tokio::io::AsyncBufReadExt;
use tokio::sync::{Mutex, broadcast};

#[derive(Embed)]
#[folder = "../frontend/dist/"]
struct Assets;

#[derive(Clone)]
struct SseEvent {
    event: String,
    data: String,
}

struct ActiveRun {
    run_id: u32,
    state: Mutex<PipelineRunState>,
    tx: broadcast::Sender<SseEvent>,
    active: AtomicBool,
}

pub(crate) struct BossState {
    pipeline: PipelineDto,
    pipedef_path: PathBuf,
    bettertest_lib_dir: PathBuf,
    run_counter: AtomicU32,
    active_run: Mutex<Option<Arc<ActiveRun>>>,
}

#[derive(Serialize)]
struct StageStartedEvent<'a> {
    stage: &'a str,
}

#[derive(Serialize)]
struct TaskResultEvent<'a> {
    stage: &'a str,
    task: &'a str,
    passed: bool,
    output: &'a str,
}

#[derive(Serialize)]
struct TaskOutputEvent<'a> {
    stage: &'a str,
    task: &'a str,
    line: &'a str,
}

fn initial_run_state(pipeline: &PipelineDto, run_id: u32) -> PipelineRunState {
    PipelineRunState {
        run_id,
        active: true,
        stages: pipeline
            .stages
            .iter()
            .map(|s| StageRunState {
                name: s.name.clone(),
                tasks: s
                    .tasks
                    .iter()
                    .map(|t| TaskRunState {
                        name: t.clone(),
                        state: TaskState::Pending,
                        output: String::new(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn parse_pipedef(path: &Path) -> PipelineDto {
    let script = include_str!("../scripts/parse_pipedef.py");
    let tmp = std::env::temp_dir().join("bettertest_parse_pipedef.py");
    std::fs::write(&tmp, script).expect("failed to write parser script to temp file");

    let output = std::process::Command::new("python3")
        .arg(&tmp)
        .arg(path)
        .output()
        .expect("failed to run python3 - is it installed?");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("pipedef parse failed:\n{stderr}");
    }

    let stdout = String::from_utf8(output.stdout).expect("python output wasn't utf8");
    let stages: Vec<StageDto> = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("failed to parse pipedef json: {e}\nraw output: {stdout}"));

    PipelineDto { stages }
}

fn setup_lib_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("bettertest_lib");
    std::fs::create_dir_all(&dir).expect("failed to create bettertest lib dir");
    std::fs::write(
        dir.join("bettertest.py"),
        include_str!("../../bettertest/__init__.py"),
    )
    .expect("failed to write bettertest.py");
    std::fs::write(
        dir.join("run_task.py"),
        include_str!("../scripts/run_task.py"),
    )
    .expect("failed to write run_task.py");
    dir
}

async fn get_state(State(state): State<Arc<BossState>>) -> Json<StateResponse> {
    let run = {
        let guard = state.active_run.lock().await;
        match guard.as_ref() {
            Some(active) => {
                let st = active.state.lock().await;
                let mut snapshot = st.clone();
                snapshot.active = active.active.load(Ordering::Relaxed);
                Some(snapshot)
            }
            None => None,
        }
    };
    Json(StateResponse {
        pipeline: state.pipeline.clone(),
        run,
    })
}

#[derive(Serialize)]
pub struct RunCreated {
    run_id: u32,
}

async fn create_run(State(state): State<Arc<BossState>>) -> Json<RunCreated> {
    let run_id = state
        .run_counter
        .fetch_add(1, Ordering::Relaxed)
        + 1;
    let (tx, _) = broadcast::channel::<SseEvent>(256);

    let run_state = initial_run_state(&state.pipeline, run_id);
    let active_run = Arc::new(ActiveRun {
        run_id,
        state: Mutex::new(run_state),
        tx: tx.clone(),
        active: AtomicBool::new(true),
    });

    *state.active_run.lock().await = Some(active_run.clone());

    let pipeline = state.pipeline.clone();
    let pipedef_path = state.pipedef_path.clone();
    let lib_dir = state.bettertest_lib_dir.clone();

    tokio::spawn(async move {
        for stage in &pipeline.stages {
            {
                let mut st = active_run.state.lock().await;
                if let Some(s) = st
                    .stages
                    .iter_mut()
                    .find(|s| s.name == stage.name)
                {
                    for task in &mut s.tasks {
                        task.state = TaskState::Running;
                    }
                }
            }
            let _ = active_run.tx.send(SseEvent {
                event: "stage_started".into(),
                data: serde_json::to_string(&StageStartedEvent { stage: &stage.name }).unwrap(),
            });

            let mut set = tokio::task::JoinSet::new();
            for task in &stage.tasks {
                let lib_dir = lib_dir.clone();
                let pipedef_path = pipedef_path.clone();
                let stage_name = stage.name.clone();
                let task_name = task.clone();
                let active_run = active_run.clone();

                set.spawn(async move {
                    let spawn_result = tokio::process::Command::new("python3")
                        .arg("-u")
                        .arg(lib_dir.join("run_task.py"))
                        .arg(&lib_dir)
                        .arg(&pipedef_path)
                        .arg(&stage_name)
                        .arg(&task_name)
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn();

                    let passed = match spawn_result {
                        Ok(mut child) => {
                            let (line_tx, mut line_rx) =
                                tokio::sync::mpsc::unbounded_channel::<String>();

                            let stdout = child.stdout.take().unwrap();
                            let tx = line_tx.clone();
                            tokio::spawn(async move {
                                let mut lines = tokio::io::BufReader::new(stdout).lines();
                                while let Ok(Some(line)) = lines.next_line().await {
                                    let _ = tx.send(line);
                                }
                            });

                            let stderr = child.stderr.take().unwrap();
                            tokio::spawn(async move {
                                let mut lines = tokio::io::BufReader::new(stderr).lines();
                                while let Ok(Some(line)) = lines.next_line().await {
                                    let _ = line_tx.send(line);
                                }
                            });

                            while let Some(line) = line_rx.recv().await {
                                {
                                    let mut st = active_run.state.lock().await;
                                    if let Some(s) = st
                                        .stages
                                        .iter_mut()
                                        .find(|s| s.name == stage_name)
                                        && let Some(t) =
                                            s.tasks.iter_mut().find(|t| t.name == task_name)
                                    {
                                        if !t.output.is_empty() {
                                            t.output.push('\n');
                                        }
                                        t.output.push_str(&line);
                                    }
                                }
                                let _ = active_run.tx.send(SseEvent {
                                    event: "task_output".into(),
                                    data: serde_json::to_string(&TaskOutputEvent {
                                        stage: &stage_name,
                                        task: &task_name,
                                        line: &line,
                                    })
                                    .unwrap(),
                                });
                            }

                            child.wait().await.is_ok_and(|s| s.success())
                        }
                        Err(e) => {
                            let mut st = active_run.state.lock().await;
                            if let Some(s) = st
                                .stages
                                .iter_mut()
                                .find(|s| s.name == stage_name)
                                && let Some(t) = s.tasks.iter_mut().find(|t| t.name == task_name)
                            {
                                t.output = e.to_string();
                            }
                            false
                        }
                    };

                    let output = {
                        let st = active_run.state.lock().await;
                        st.stages
                            .iter()
                            .find(|s| s.name == stage_name)
                            .and_then(|s| s.tasks.iter().find(|t| t.name == task_name))
                            .map(|t| t.output.clone())
                            .unwrap_or_default()
                    };

                    {
                        let mut st = active_run.state.lock().await;
                        if let Some(s) = st
                            .stages
                            .iter_mut()
                            .find(|s| s.name == stage_name)
                            && let Some(t) = s.tasks.iter_mut().find(|t| t.name == task_name)
                        {
                            t.state = if passed {
                                TaskState::Pass
                            } else {
                                TaskState::Fail
                            };
                        }
                    }

                    let _ = active_run.tx.send(SseEvent {
                        event: "task_result".into(),
                        data: serde_json::to_string(&TaskResultEvent {
                            stage: &stage_name,
                            task: &task_name,
                            passed,
                            output: &output,
                        })
                        .unwrap(),
                    });
                });
            }
            while set.join_next().await.is_some() {}
        }

        let _ = active_run.tx.send(SseEvent {
            event: "run_done".into(),
            data: "{}".into(),
        });
        active_run
            .active
            .store(false, Ordering::Relaxed);
    });

    Json(RunCreated { run_id })
}

async fn run_events(
    State(state): State<Arc<BossState>>,
    AxumPath(run_id): AxumPath<u32>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let active_run = {
        let guard = state.active_run.lock().await;
        match guard
            .as_ref()
            .filter(|r| r.run_id == run_id)
            .cloned()
        {
            Some(r) => r,
            None => return Err(StatusCode::NOT_FOUND),
        }
    };

    // subscribe BEFORE snapshot — no gap
    let mut rx = active_run.tx.subscribe();

    let snapshot = {
        let st = active_run.state.lock().await;
        serde_json::to_string(&*st).unwrap()
    };

    let stream = async_stream::stream! {
        yield Ok(Event::default().event("state").data(snapshot));

        loop {
            match rx.recv().await {
                Ok(sse) => {
                    let done = sse.event == "run_done";
                    yield Ok(Event::default().event(sse.event).data(sse.data));
                    if done { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let fresh = {
                        let st = active_run.state.lock().await;
                        serde_json::to_string(&*st).unwrap()
                    };
                    yield Ok(Event::default().event("state").data(fresh));
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream))
}

async fn serve_asset(path: &str) -> Response {
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref())],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

async fn index() -> Response {
    serve_asset("index.html").await
}

async fn debug_page() -> Response {
    let html = include_str!("../../frontend/debug.html");
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html")], html).into_response()
}

async fn static_files(AxumPath(path): AxumPath<String>) -> Response {
    serve_asset(&path).await
}

fn api_routes() -> Router<Arc<BossState>> {
    Router::new()
        .route("/api/state", get(get_state))
        .route("/api/run", post(create_run))
        .route("/api/run/{id}/events", get(run_events))
}

fn static_routes() -> Router<Arc<BossState>> {
    Router::new()
        .route("/", get(index))
        .route("/logs", get(index))
        .route("/debug", get(debug_page))
        .route("/{*path}", get(static_files))
}

pub async fn run(pipedef_path: &Path) {
    let pipeline = parse_pipedef(pipedef_path);
    println!("parsed pipedef: {} stages", pipeline.stages.len());
    for stage in &pipeline.stages {
        println!("  {} ({} tasks)", stage.name, stage.tasks.len());
    }

    let lib_dir = setup_lib_dir();
    println!("bettertest lib dir: {}", lib_dir.display());

    let state = Arc::new(BossState {
        pipeline,
        pipedef_path: pipedef_path.to_path_buf(),
        bettertest_lib_dir: lib_dir,
        run_counter: AtomicU32::new(0),
        active_run: Mutex::new(None),
    });

    let app = api_routes()
        .merge(static_routes())
        .with_state(state);

    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_reuseaddr(true).unwrap();
    socket
        .bind("0.0.0.0:9001".parse().unwrap())
        .unwrap();
    let listener = socket.listen(1024).unwrap();
    println!("bettertest boss running on http://localhost:9001");
    axum::serve(listener, app).await.unwrap();
}
