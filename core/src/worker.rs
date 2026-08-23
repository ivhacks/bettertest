use std::error::Error;

use axum::{Router, routing::get};

use tokio::net::TcpListener;

const LISTEN_PORT: u16 = 9010;

pub async fn entry() -> Result<(), Box<dyn Error>> {
    let router = Router::new().route("/health", get(health));

    println!("Worker API at http://[::1]:{LISTEN_PORT}");
    let listener = TcpListener::bind(("::", LISTEN_PORT)).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}
