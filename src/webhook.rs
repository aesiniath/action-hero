//! This is a module to receive webhooks from GitHub when a GitHub Action
//! workflow is run.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Router, routing::get};
use serde::Deserialize;
use tracing::info;

use crate::github::{self, Config};

pub(crate) async fn run_webserver(port: u32) -> anyhow::Result<()> {
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
    workflow_run: github::WorkflowRun,
}

#[derive(Deserialize)]
struct WebhookOrganization {
    login: String,
}

#[derive(Deserialize)]
struct WebhookRepository {
    name: String,
}

async fn hello_world() -> &'static str {
    "Hello world!"
}

// Make a wrapper around `anyhow::Error`.
struct ErrorWrapper(anyhow::Error);

// Tell axum how to convert that wrapper into a response.
impl IntoResponse for ErrorWrapper {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", self.0)).into_response()
    }
}

impl From<anyhow::Error> for ErrorWrapper {
    fn from(error: anyhow::Error) -> Self {
        ErrorWrapper(error)
    }
}

/// Handler for incoming webhook requests. This will extract the supplied
/// WorkflowRun, fire off the query to get its jobs and steps, then process
/// that into telemetry.
async fn receive_post(Json(payload): Json<RequestPayload>) -> Result<(), ErrorWrapper> {
    let path = payload
        .workflow_run
        .path
        .clone();
    let filename = path
        .split('/')
        .last()
        .unwrap()
        .to_string();

    // This served as a useful diagnostic to ensure we had the right fields
    // from the inbound request's JSON object body.

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
            .clone()
            .unwrap_or("null".to_string())
    );

    // Now use those fields to form the Config object that will be used to
    // drive processing the run.

    let config = Config {
        owner: payload
            .organization
            .login,
        repository: payload
            .repository
            .name,
        workflow: filename,
        devel: false,
    };

    let client = github::setup_api_client()?;

    let result = crate::process_run(&config, &client, &payload.workflow_run).await;

    // if there was a problem wrap it in the adapter type so we get something
    // that converts via IntoResponse.
    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(ErrorWrapper(err)),
    }
}
