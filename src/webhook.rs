//! This is a module to receive webhooks from GitHub when a GitHub Action
//! workflow is run.

use anyhow::Result;
use axum::Json;
use axum::{Router, routing::get};
use serde::Deserialize;
use tracing::info;

pub(crate) async fn run_webserver(port: u32) -> Result<()> {
    let router = Router::new().route("/", get(hello_world).post(receive_post));

    let address = format!("127.0.0.1:{}", port);
    info!("Listening on {}", address);

    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, router).await?;

    Ok(())
}

#[derive(Deserialize)]
struct RequestPayload {
    action: String,
    organization: WebhookOrganization,
    repository: WebhookRepository,
    workflow_run: WebhookWorkflowRun,
}

#[derive(Deserialize)]
struct WebhookOrganization {
    login: String,
}

#[derive(Deserialize)]
struct WebhookRepository {
    name: String,
}

#[derive(Deserialize)]
struct WebhookWorkflowRun {
    actor: WebhookActor,
    conclusion: String,
    display_title: String,
    event: String,
    head_branch: String,
    path: String,
}

#[derive(Deserialize)]
struct WebhookActor {
    login: String,
}

async fn hello_world() -> &'static str {
    "Hello world!"
}

async fn receive_post(Json(payload): Json<RequestPayload>) {
    let path = payload
        .workflow_run
        .path;
    let filename = path
        .split('/')
        .last()
        .unwrap();

    println!(
        "{}: {}/{} {} \"{}\" by {} via {} for {}: {}",
        payload.action,
        payload
            .organization
            .login,
        payload
            .repository
            .name,
        filename,
        payload
            .workflow_run
            .display_title,
        payload
            .workflow_run
            .actor
            .login,
        payload
            .workflow_run
            .event,
        payload
            .workflow_run
            .head_branch,
        payload
            .workflow_run
            .conclusion
    );
}
