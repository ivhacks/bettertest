use std::convert::Infallible;

use aide::axum::{ApiRouter, routing::{get_with, post_with}};
use aide::NoApi;
use axum::{
    Json,
    response::sse::{Event, Sse},
};
use bollard::Docker;
use bollard::query_parameters::CreateContainerOptions;
use bollard::secret::ContainerCreateBody;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;

type EventStream = Sse<ReceiverStream<Result<Event, Infallible>>>;

#[derive(Deserialize, JsonSchema)]
pub struct RunRequest {
    image: String,
    command: String,
}

async fn health() -> &'static str {
    "ok\n"
}

async fn handle_run(docker: Docker, Json(req): Json<RunRequest>) -> NoApi<EventStream> {
    let (tx, rx) = tokio::sync::mpsc::channel(16);

    tokio::spawn(async move {
        let id = match docker
            .create_container(
                None::<CreateContainerOptions>,
                ContainerCreateBody {
                    image: Some(req.image),
                    cmd: Some(vec!["sh".into(), "-c".into(), req.command]),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(c) => c.id,
            Err(e) => {
                let _ = tx
                    .send(Ok(Event::default().event("error").data(e.to_string())))
                    .await;
                return;
            }
        };

        docker.start_container(&id, None).await.ok();
        let _ = tx
            .send(Ok(Event::default().event("started").data(id.clone())))
            .await;

        // stream logs (follow=true blocks until container exits)
        let log_opts = bollard::query_parameters::LogsOptions {
            follow: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        };
        let mut logs = docker.logs(&id, Some(log_opts));
        while let Some(Ok(log)) = logs.next().await {
            for line in log.to_string().lines() {
                let _ = tx.send(Ok(Event::default().event("log").data(line))).await;
            }
        }

        // container is stopped now, inspect to get exit code
        let exit_code = docker
            .inspect_container(&id, None)
            .await
            .ok()
            .and_then(|info| info.state)
            .and_then(|state| state.exit_code)
            .unwrap_or(-1);

        let _ = tx
            .send(Ok(Event::default()
                .event("done")
                .data(exit_code.to_string())))
            .await;
        docker.remove_container(&id, None).await.ok();
    });

    NoApi(Sse::new(ReceiverStream::new(rx)))
}

pub fn api_routes(docker: Docker) -> ApiRouter {
    ApiRouter::new()
        .api_route("/health", get_with(health, |op| op))
        .api_route("/run", post_with(move |req: Json<RunRequest>| handle_run(docker, req), |op| op))
}

pub async fn run() {
    let docker = Docker::connect_with_local_defaults().expect("failed to connect to docker");

    let mut api = aide::openapi::OpenApi::default();
    let app = api_routes(docker)
        .finish_api(&mut api);

    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_reuseaddr(true).unwrap();
    socket.bind("0.0.0.0:9009".parse().unwrap()).unwrap();
    let listener = socket.listen(1024).unwrap();
    println!("bettertest worker running on http://0.0.0.0:9009");
    axum::serve(listener, app).await.unwrap();
}
