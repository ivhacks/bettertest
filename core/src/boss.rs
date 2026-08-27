use crate::db::*;
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
use bettertest_shared_crate::*;
use rust_embed::Embed;
use std::{
    convert::Infallible,
    error::Error,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};
use tokio::{net::TcpListener, spawn, sync::broadcast};
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use uuid::Uuid;

struct CurrentPipedef {
    pipeline: Pipeline,
    source: String,
}

#[derive(Clone)]
struct BossState {
    current: Arc<Mutex<CurrentPipedef>>,
    db: Db,
    http: reqwest::Client,
    events: broadcast::Sender<(String, String)>,
}

#[derive(Embed)]
#[folder = "../frontend/dist/"]
struct EmbeddedWebAssets;

const LISTEN_PORT: u16 = 9009;

impl BossState {
    fn emit(&self, event: &str, data: impl serde::Serialize) {
        let _ = self
            .events
            .send((event.to_string(), serde_json::to_string(&data).unwrap()));
    }
}

pub async fn entry(pipedef: PathBuf) -> Result<(), Box<dyn Error>> {
    let pipeline = crate::pipedef::parse(&pipedef).unwrap();
    let source = std::fs::read_to_string(&pipedef).unwrap();
    let (events, _) = broadcast::channel(64);
    let current = Arc::new(Mutex::new(CurrentPipedef { pipeline, source }));
    watch_pipedef(pipedef.clone(), current.clone(), events.clone());
    let state = BossState {
        current,
        db: Db::open(),
        http: reqwest::Client::new(),
        events,
    };
    let router = Router::new()
        .route("/", get(handle_get_index))
        .route("/index.html", get(handle_get_index))
        .route("/logs", get(handle_get_index))
        .route("/api/health", get(handle_get_health))
        .route("/api/state", get(handle_get_state))
        .route("/api/events", get(handle_get_events))
        .route("/api/logs/{task}", get(handle_get_logs))
        .route("/api/run", post(handle_post_start_run))
        .route("/{*path}", get(handle_get_embedded_asset))
        .with_state(state);

    println!("Boss web UI: http://[::1]:{LISTEN_PORT}");
    let listener = TcpListener::bind(("::", LISTEN_PORT)).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn handle_get_state(State(state): State<BossState>) -> Json<StateResponse> {
    let pipeline = state.current.lock().unwrap().pipeline.clone();
    Json(StateResponse {
        pipeline,
        runs: state.db.list_runs(),
    })
}

async fn handle_get_logs(
    State(state): State<BossState>,
    AxumPath(task): AxumPath<Uuid>,
) -> Json<TaskLog> {
    Json(state.db.task_log(task))
}

async fn handle_get_events(
    State(state): State<BossState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    Sse::new(
        BroadcastStream::new(state.events.subscribe()).filter_map(|item| match item {
            Ok((event, data)) => Some(Ok(Event::default().event(event).data(data))),
            Err(_) => None,
        }),
    )
}

async fn handle_post_start_run(State(state): State<BossState>) -> Json<Run> {
    let (pipeline, pipedef_source) = {
        let current = state.current.lock().unwrap();
        (current.pipeline.clone(), current.source.clone())
    };
    let run = state.db.create_run(&pipeline);
    state.emit("run", &run);
    let mut work = run.clone();
    spawn(async move {
        let run = &mut work;
        for si in 0..run.stages.len() {
            for ti in 0..run.stages[si].tasks.len() {
                let task_id = run.stages[si].tasks[ti].id;
                let task_name = run.stages[si].tasks[ti].name.clone();
                let stage_name = run.stages[si].name.clone();
                if run.stages[si].tasks[ti].state == TaskState::Disabled {
                    continue;
                }
                run.stages[si].tasks[ti].state = TaskState::Running;
                state.db.set_task_running(task_id);
                state.emit("run", &run);
                let worker = &pipeline
                    .stages
                    .iter()
                    .find(|s| s.name == stage_name)
                    .unwrap()
                    .tasks
                    .iter()
                    .find(|t| t.name == task_name)
                    .unwrap()
                    .worker;
                let form = reqwest::multipart::Form::new()
                    .text("pipedef", pipedef_source.clone())
                    .text("stage_name", stage_name)
                    .text("task_name", task_name);
                let mut resp = state
                    .http
                    .post(format!("{}/start-task", worker.trim_end_matches('/')))
                    .multipart(form)
                    .send()
                    .await
                    .unwrap();
                let mut buf = String::new();
                let mut event = String::new();
                let mut code = -1;
                loop {
                    let Some(chunk) = resp.chunk().await.unwrap() else {
                        break;
                    };
                    buf.push_str(std::str::from_utf8(&chunk).unwrap());
                    while let Some(idx) = buf.find('\n') {
                        let mut line: String = buf.drain(..=idx).collect();
                        line.pop();
                        if line.ends_with('\r') {
                            line.pop();
                        }
                        if let Some(name) = line.strip_prefix("event:") {
                            event = name.trim().to_string();
                        } else if let Some(data) = line.strip_prefix("data:") {
                            let data = data.trim_start();
                            if event == "stdout" || event == "stderr" {
                                state.db.append_log(task_id, data);
                                state.emit(
                                    "log",
                                    LogLine {
                                        task_id,
                                        line: data.to_string(),
                                    },
                                );
                            } else if event == "status" {
                                code = data.parse().unwrap();
                            }
                        }
                    }
                }
                let passed = code == 0;
                run.stages[si].tasks[ti].state = if passed {
                    TaskState::Pass
                } else {
                    TaskState::Fail
                };
                state.db.set_task_finished(task_id, passed);
                state.emit("run", &run);
            }
        }
        state.db.set_run_finished(run.id);
        state.emit("run", &*run);
    });
    Json(run)
}

async fn handle_get_health() -> &'static str {
    "ok"
}

async fn handle_get_index() -> impl IntoResponse {
    embedded_file_response("index.html", "no-cache")
}

async fn handle_get_embedded_asset(AxumPath(path): AxumPath<String>) -> Response {
    embedded_file_response(path.as_str(), "public, max-age=31536000, immutable")
}

fn watch_pipedef(
    path: PathBuf,
    current: Arc<Mutex<CurrentPipedef>>,
    events: broadcast::Sender<(String, String)>,
) {
    let path = path.canonicalize().unwrap();
    let dir = path.parent().unwrap().to_path_buf();
    thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(tx).unwrap();
        notify::Watcher::watch(&mut watcher, &dir, notify::RecursiveMode::NonRecursive).unwrap();
        for event in rx {
            let event = event.unwrap();
            let ours = event
                .paths
                .iter()
                .any(|p| p == &path || p.canonicalize().ok().as_deref() == Some(path.as_path()));
            if !ours {
                continue;
            }
            match crate::pipedef::parse(&path) {
                Ok(pipeline) => {
                    let source = std::fs::read_to_string(&path).unwrap();
                    let mut current = current.lock().unwrap();
                    if current.source == source {
                        continue;
                    }
                    current.pipeline = pipeline.clone();
                    current.source = source;
                    drop(current);
                    let _ =
                        events.send(("pipeline".into(), serde_json::to_string(&pipeline).unwrap()));
                }
                Err(e) => eprintln!("{e}"),
            }
        }
    });
}

fn embedded_file_response(path: &str, cache: &str) -> Response {
    match EmbeddedWebAssets::get(path) {
        Some(content) => {
            let mime = content.metadata.mimetype();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, cache)],
                content.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
