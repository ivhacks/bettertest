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
use std::{convert::Infallible, error::Error, path::PathBuf};
use tokio::{
    net::TcpListener,
    spawn,
    sync::{broadcast, mpsc},
};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};

#[derive(Clone)]
struct BossState {
    pipeline: Pipeline,
    pipedef_source: String,
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
    let pipeline = crate::pipedef::parse(&pipedef);
    let pipedef_source = std::fs::read_to_string(&pipedef).unwrap();
    let (events, _) = broadcast::channel(64);
    let state = BossState {
        pipeline,
        pipedef_source,
        db: Db::open(),
        http: reqwest::Client::new(),
        events,
    };
    let router = Router::new()
        .route("/", get(handle_get_index))
        .route("/index.html", get(handle_get_index))
        .route("/api/health", get(handle_get_health))
        .route("/api/pipeline", get(handle_get_pipeline))
        .route("/api/state", get(handle_get_state))
        .route("/api/events", get(handle_get_events))
        .route("/api/run", post(handle_post_start_run))
        .route("/{*path}", get(handle_get_embedded_asset))
        .with_state(state);

    println!("Boss web UI: http://[::1]:{LISTEN_PORT}");
    let listener = TcpListener::bind(("::", LISTEN_PORT)).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn handle_get_pipeline(State(state): State<BossState>) -> Json<Pipeline> {
    Json(state.pipeline)
}

async fn handle_get_state(State(state): State<BossState>) -> Json<StateResponse> {
    Json(StateResponse {
        pipeline: state.pipeline,
        runs: state.db.list_runs(),
    })
}

async fn handle_get_events(
    State(state): State<BossState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.events.subscribe();
    let (tx, out) = mpsc::channel(64);
    spawn(async move {
        loop {
            match rx.recv().await {
                Ok((event, data)) => {
                    if tx
                        .send(Event::default().event(event).data(data))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Sse::new(ReceiverStream::new(out).map(Ok::<_, Infallible>))
}

async fn handle_post_start_run(State(state): State<BossState>) -> Json<RunResponse> {
    let run = state.db.create_run(&state.pipeline);
    let id = run.id;
    state.emit("run_started", &run);

    spawn(async move {
        for stage in &run.stages {
            for task in &stage.tasks {
                state.db.set_task_running(task.id);
                state.emit(
                    "task",
                    TaskUpdate {
                        run_id: id,
                        task_id: task.id,
                        state: TaskState::Running,
                    },
                );
                let worker = &state
                    .pipeline
                    .stages
                    .iter()
                    .find(|s| s.name == stage.name)
                    .unwrap()
                    .tasks
                    .iter()
                    .find(|t| t.name == task.name)
                    .unwrap()
                    .worker;
                let form = reqwest::multipart::Form::new()
                    .text("pipedef", state.pipedef_source.clone())
                    .text("stage_name", stage.name.clone())
                    .text("task_name", task.name.clone());
                let body = state
                    .http
                    .post(format!("{}/start-task", worker.trim_end_matches('/')))
                    .multipart(form)
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                let mut log = String::new();
                let mut event = String::new();
                let mut code = -1;
                for line in body.lines() {
                    if let Some(name) = line.strip_prefix("event:") {
                        event = name.trim().to_string();
                    } else if let Some(data) = line.strip_prefix("data:") {
                        let data = data.trim_start();
                        if event == "stdout" || event == "stderr" {
                            if !log.is_empty() {
                                log.push('\n');
                            }
                            log.push_str(data);
                        } else if event == "status" {
                            code = data.parse().unwrap();
                        }
                    }
                }
                let passed = code == 0;
                state.db.set_task_finished(task.id, passed, &log);
                state.emit(
                    "task",
                    TaskUpdate {
                        run_id: id,
                        task_id: task.id,
                        state: if passed {
                            TaskState::Pass
                        } else {
                            TaskState::Fail
                        },
                    },
                );
            }
        }
        state.db.set_run_finished(run.id);
        state.emit("run_done", RunDone { run_id: id });
    });
    Json(RunResponse { id })
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
