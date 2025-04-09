//! This is a module to receive webhooks from GitHub when a GitHub Action
//! workflow is run.

use anyhow::Result;
use axum::Json;
use axum::{Router, routing::get};
use tracing::debug;
use tracing::info;

use crate::VERSION;

pub(crate) async fn run_webserver(port: u32) -> Result<()> {
    let router = Router::new().route("/", get(hello_world).post(receive_post));

    let address = format!("127.0.0.1:{}", port);
    info!("Listening on {}", address);

    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, router).await?;

    Ok(())
}

async fn hello_world() -> &'static str {
    "Hello world!"
}

async fn receive_post(Json(value): Json<serde_json::Value>) {
    println!("{}", serde_json::to_string_pretty(&value).unwrap())
}
