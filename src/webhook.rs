//! This is a module to receive webhooks from GitHub when a GitHub Action
//! workflow is run.

use std::f64::consts::PI;

use anyhow::Result;
use axum::{Router, routing::get};
use tracing::debug;
use tracing::info;

use crate::VERSION;

pub(crate) async fn run_webserver() -> Result<()> {
    let router = Router::new().route("/", get(hello_world));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:34484").await?;
    axum::serve(listener, router).await?;

    Ok(())
}

async fn hello_world() -> &'static str {
    "Hello world!"
}
