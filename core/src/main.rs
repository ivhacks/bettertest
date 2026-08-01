use axum::http::{StatusCode, header};
use axum::response::Response;
use rust_embed::Embed;
use std::convert::Infallible;
use std::process::Stdio;
use tokio::io::BufReader;
use tokio::spawn;
use tokio::{io::AsyncBufReadExt, process::Command, sync::mpsc};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};

use axum::{
    Json, Router,
    extract::Path as AxumPath,
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use serde::Deserialize;

use bettertest_shared_crate::*;

#[derive(Embed)]
#[folder = "../frontend/dist/"]
struct EmbeddedWebAssets;

#[derive(Deserialize)]
struct RunTaskRequest {
    command: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:9009").await?;

    let router = Router::<()>::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/api/health", get(health))
        .route("/api/run-task", post(run_task))
        .route("/api/pipeline", get(static_dummy_pipeline))
        .route("/{*path}", get(get_embedded_asset));
    axum::serve(listener, router).await?;
    Ok(())
}

#[axum::debug_handler]
async fn static_dummy_pipeline() -> Json<PipelineResponse> {
    Json(PipelineResponse {
        name: "sausage sucker 9000 turbo GTS".to_string(),
        stage_headers: vec![
            "suck".to_string(),
            "slurp".to_string(),
            "slobber".to_string(),
        ],
        runs: vec![
            Run {
                id: 1,
                active: false,
                stages: vec![
                    Stage {
                        name: "suck".to_string(),
                        tasks: vec![Task {
                            name: "gurt".to_string(),
                        }],
                    },
                    Stage {
                        name: "slurp".to_string(),
                        tasks: vec![Task {
                            name: "gurt".to_string(),
                        }],
                    },
                    Stage {
                        name: "slobber".to_string(),
                        tasks: vec![Task {
                            name: "gurt".to_string(),
                        }],
                    },
                ],
            },
            Run {
                id: 2,
                active: false,
                stages: vec![
                    Stage {
                        name: "suck".to_string(),
                        tasks: vec![Task {
                            name: "gurt".to_string(),
                        }],
                    },
                    Stage {
                        name: "slurp".to_string(),
                        tasks: vec![Task {
                            name: "gurt".to_string(),
                        }],
                    },
                    Stage {
                        name: "slobber".to_string(),
                        tasks: vec![Task {
                            name: "gurt".to_string(),
                        }],
                    },
                ],
            },
        ],
        pipelines: vec!["build".to_string(), "inception".to_string()],
    })
}

async fn health() -> &'static str {
    "ok"
}

async fn run_task(Json(req): Json<RunTaskRequest>) -> impl IntoResponse {
    let (return_tx, rx) = mpsc::channel::<Event>(64);
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&req.command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let child_stdout = child.stdout.take().unwrap();
    let child_stderr = child.stderr.take().unwrap();

    let stdout_tx = return_tx.clone();
    let mut stdout_lines = BufReader::new(child_stdout).lines();
    let stdout_task = spawn(async move {
        while let Ok(Some(line)) = stdout_lines.next_line().await {
            if stdout_tx
                .send(Event::default().event("stdout").data(line))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let stderr_tx = return_tx.clone();
    let mut stderr_lines = BufReader::new(child_stderr).lines();
    let stderr_task = spawn(async move {
        while let Ok(Some(line)) = stderr_lines.next_line().await {
            if stderr_tx
                .send(Event::default().event("stderr").data(line))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    spawn(async move {
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        if let Ok(exit_status) = child.wait().await
            && let Some(rc) = exit_status.code()
        {
            let _ = return_tx
                .send(Event::default().event("status").data(rc.to_string()))
                .await;
        }
    });

    Sse::new(ReceiverStream::new(rx).map(Ok::<_, Infallible>))
}

async fn index() -> impl IntoResponse {
    embedded_file_response("index.html", "no-cache")
}

async fn get_embedded_asset(AxumPath(path): AxumPath<String>) -> Response {
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
