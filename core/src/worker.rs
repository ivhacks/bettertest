use crate::dtos::*;
use crate::embedded_scripts::*;
use axum::{
    Router,
    response::{IntoResponse, sse::*},
    routing::*,
};
use axum_typed_multipart::*;
use std::{convert::*, error::Error, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::*,
    process::*,
    spawn,
    sync::mpsc,
};
use tokio_stream::{StreamExt, wrappers::*};

const LISTEN_PORT: u16 = 9010;

pub async fn entry() -> Result<(), Box<dyn Error>> {
    let router = Router::new()
        .route("/health", get(health))
        .route("/start-task", post(start_task));

    println!("Worker API at http://[::1]:{LISTEN_PORT}");
    let listener = TcpListener::bind(("::", LISTEN_PORT)).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn pump(
    reader: impl tokio::io::AsyncRead + Unpin + Send,
    tx: mpsc::Sender<Event>,
    event: &'static str,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        tx.send(Event::default().event(event).data(line))
            .await
            .unwrap();
    }
}

async fn start_task(TypedMultipart(req): TypedMultipart<StartTaskRequest>) -> impl IntoResponse {
    let (tx, rx) = mpsc::channel::<Event>(64);
    let mut child = Command::new("python3")
        .arg("-u")
        .arg("-c")
        .arg(GLUE_SCRIPT)
        .arg("start")
        .arg(&req.stage_name)
        .arg(&req.task_name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let payload = stdin_payload(&req.pipedef_content);

    spawn(async move {
        stdin.write_all(&payload).await.unwrap();
        drop(stdin);
        tokio::join!(
            pump(stdout, tx.clone(), "stdout"),
            pump(stderr, tx.clone(), "stderr"),
        );
        let rc = child.wait().await.unwrap().code().unwrap();
        tx.send(Event::default().event("status").data(rc.to_string()))
            .await
            .unwrap();
    });

    Sse::new(ReceiverStream::new(rx).map(Ok::<_, Infallible>))
}
