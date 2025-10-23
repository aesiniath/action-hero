//! This is a module to receive webhooks from GitHub when a GitHub Action
//! workflow is run.

use std::net::Ipv4Addr;

use anyhow::anyhow;
use axum::Json;
use axum::body::Body;
use axum::extract::FromRequest;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Router, routing::get};
use serde::Deserialize;
use tracing::info;

use crate::github::{self, Config};

pub(crate) async fn run_webserver(host: Ipv4Addr, port: u16) -> anyhow::Result<()> {
    let router = Router::new().route("/", get(hello_world).post(receive_post));

    info!("Listening on {:?}:{}", host, port);
    let address = (host, port);

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

// Make a wrapper around `anyhow::Error` and other branching escape paths we
// want to convert into specific response codes.
enum ErrorWrapper {
    AnyhowError(anyhow::Error),
    MissingHeader,
    IgnoredType(String),
    IgnoredAction(String),
    JsonFailure(axum::extract::rejection::JsonRejection),
}

// Tell axum how to convert that wrapper into a response.
impl IntoResponse for ErrorWrapper {
    fn into_response(self) -> Response {
        match self {
            ErrorWrapper::AnyhowError(error) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", error)).into_response()
            }
            ErrorWrapper::MissingHeader => {
                (StatusCode::BAD_REQUEST, "Missing X-GitHub-Event header").into_response()
            }
            ErrorWrapper::IgnoredType(what) => {
                (
                    StatusCode::NON_AUTHORITATIVE_INFORMATION,
                    format!("Ignoring '{}' event", what),
                )
                    .into_response() // such a stupid field name
            }
            ErrorWrapper::IgnoredAction(what) => {
                (
                    StatusCode::NON_AUTHORITATIVE_INFORMATION,
                    format!("Ignoring '{}' action", what),
                )
                    .into_response() // such a stupid field name
            }
            ErrorWrapper::JsonFailure(problem) => {
                (StatusCode::UNPROCESSABLE_ENTITY, problem).into_response()
            }
        }
    }
}

impl From<anyhow::Error> for ErrorWrapper {
    fn from(error: anyhow::Error) -> Self {
        ErrorWrapper::AnyhowError(error)
    }
}

struct GitHubEvent(Json<RequestPayload>);

impl<S> FromRequest<S> for GitHubEvent
where
    S: Send + Sync,
{
    type Rejection = ErrorWrapper;

    async fn from_request(req: Request<Body>, state: &S) -> Result<Self, Self::Rejection> {
        if let Some(event) = req
            .headers()
            .get("X-GitHub-Event")
        {
            if event != "workflow_run" {
                return Err(ErrorWrapper::IgnoredType(
                    event
                        .to_str()
                        .unwrap()
                        .to_owned(),
                ));
            }
            let result = Json::<RequestPayload>::from_request(req, state).await;
            match result {
                Ok(json) => Ok(GitHubEvent(json)),
                Err(problem) => Err(ErrorWrapper::JsonFailure(problem)),
            }
        } else {
            return Err(ErrorWrapper::MissingHeader);
        }
    }
}

/// Handler for incoming webhook requests. This will extract the supplied
/// WorkflowRun, fire off the query to get its jobs and steps, then process
/// that into telemetry.
async fn receive_post(GitHubEvent(payload): GitHubEvent) -> Result<(), ErrorWrapper> {
    let path = payload
        .workflow_run
        .path
        .clone();
    let filename = path
        .split('/')
        .last()
        .ok_or(anyhow!("Could not get Filename"))?
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

    // make a decision about whether this is a request we can handle

    if payload.action != "completed" {
        return Err(ErrorWrapper::IgnoredAction(
            payload
                .action
                .clone(),
        ));
    }

    // Now use those fields to form the Config object that will be used to
    // drive processing the run.

    let config = Config {
        owner: payload
            .organization
            .login
            .clone(),
        repository: payload
            .repository
            .name
            .clone(),
        workflow: filename,
        devel: false,
    };

    let client = github::setup_api_client()?;

    let result = crate::process_run(&config, &client, &payload.workflow_run).await;

    // if there was a problem wrap it in the adapter type so we get something
    // that converts via IntoResponse.
    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(ErrorWrapper::AnyhowError(err)),
    }
}
