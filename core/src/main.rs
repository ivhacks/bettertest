use axum::http::{StatusCode, header};
use rust_embed::Embed;
use std::convert::Infallible;
use std::process::Stdio;
use tokio::io::BufReader;
use tokio::spawn;
use tokio::{io::AsyncBufReadExt, process::Command, sync::mpsc};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};

use axum::{
    Json, Router,
    response::{IntoResponse, Sse, sse::Event},
    routing::{get, post},
};
use serde::Deserialize;

#[derive(Embed)]
#[folder = "../frontend/dist/"]
struct Asset;

#[derive(Deserialize)]
struct RunTaskRequest {
    command: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:9009").await?;
    let router = Router::<()>::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/run-task", post(run_task));
    axum::serve(listener, router).await?;
    Ok(())
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
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Asset::get("fake_index.html").unwrap().data.clone(),
    )
        .into_response()
}
