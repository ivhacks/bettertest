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
use serde::Deserialize;
use std::{convert::Infallible, error::Error, path::PathBuf, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpListener,
    process::Command,
    spawn,
    sync::mpsc,
};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use uuid::Uuid;

#[derive(Embed)]
#[folder = "../frontend/dist/"]
struct EmbeddedWebAssets;

#[derive(Deserialize)]
struct RunTaskRequest {
    command: String,
}

const LISTEN_PORT: u16 = 9009;

pub async fn entry(pipedef: PathBuf) -> Result<(), Box<dyn Error>> {
    let parsed = crate::pipedef::parse(&pipedef);
    let router = Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/api/health", get(health))
        .route("/api/run-task", post(run_task))
        .route("/api/goofball", post(dummy_run))
        .route("/api/pipeline", get(get_pipeline))
        .route("/{*path}", get(get_embedded_asset))
        .with_state(parsed);

    println!("Boss web UI: http://[::1]:{LISTEN_PORT}");
    let listener = TcpListener::bind(("::", LISTEN_PORT)).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn dummy_run() -> Json<RunResponse> {
    let new_run = RunResponse { id: Uuid::new_v4() };
    Json(new_run)
}

async fn get_pipeline(State(pipeline): State<Pipeline>) -> Json<Pipeline> {
    Json(pipeline)
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
