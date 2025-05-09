use anyhow::Result;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use time::Duration;
use time::OffsetDateTime;
use time::serde::rfc3339;
use tracing::info;
use tracing::{debug, warn};

use crate::VERSION;
use crate::{get_api_token, get_program_start};

/// A struct holding the configuration being used to retrieve information from
/// GitHub's API.
pub(crate) struct Config {
    pub(crate) owner: String,
    pub(crate) repository: String,
    pub(crate) workflow: String,
    pub(crate) devel: bool,
}

// We have structs for all the relevant objects in the GitHub API. This was
// initially created by the responses for the various GitHub Actions Workflow
// Run responses, but it turns out the payload for the webhook is the same
// object, so we were able to re-use this.

#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowRun {
    pub(crate) actor: WorkflowActor,
    #[serde(rename = "id")]
    pub(crate) run_id: u64,
    pub(crate) run_number: u64,
    pub(crate) run_attempt: u64,
    pub(crate) head_branch: String,
    pub(crate) name: String,
    pub(crate) display_title: String,
    pub(crate) event: String, // what caused the workflow to run
    pub(crate) status: String,
    pub(crate) conclusion: Option<String>,
    #[serde(with = "rfc3339")]
    pub(crate) created_at: OffsetDateTime,
    #[serde(with = "rfc3339")]
    pub(crate) updated_at: OffsetDateTime,
    pub(crate) html_url: String,
    pub(crate) path: String, // the full path and version of the workflow code

    // and now our fields that are NOT in the response object
    #[serde(default)]
    pub(crate) delta: Duration,
}
#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowActor {
    pub(crate) login: String,
}

#[derive(Deserialize)]
struct ResponseRuns {
    workflow_runs: Vec<WorkflowRun>,
}

pub(crate) async fn retrieve_workflow_runs(
    config: &Config,
    client: &reqwest::Client,
    count: u32,
) -> Result<Vec<WorkflowRun>> {
    // use token to retrieve runs for the given workflow from GitHub API
    info!("List Runs for Workflow {}", config.workflow);

    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/workflows/{}/runs?per_page={}&page=1",
        config.owner, config.repository, config.workflow, count
    );
    debug!(?url);

    let response = client
        .get(&url)
        .send()
        .await?;

    // retrieve the run ID of the most recent 10 runs
    let body: ResponseRuns = response
        .json()
        .await?;

    let mut runs: Vec<WorkflowRun> = body.workflow_runs;

    for run in runs.iter_mut() {
        // calculate the change to the origin time if we are in development
        // mode. This delta will be added to all timestamps to bring them to
        // near program start time (ie now).
        let delta = if config.devel {
            let program_start = *get_program_start();
            program_start - run.created_at - Duration::minutes(10)
        } else {
            Duration::ZERO
        };
        run.delta = delta;
    }

    Ok(runs)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkflowJob {
    #[serde(rename = "id")]
    pub(crate) job_id: u64,
    pub(crate) name: String,
    pub(crate) head_branch: String,
    pub(crate) status: String,
    pub(crate) conclusion: String,
    #[serde(with = "rfc3339")]
    pub(crate) started_at: OffsetDateTime,
    #[serde(with = "rfc3339")]
    pub(crate) completed_at: OffsetDateTime,
    pub(crate) steps: Vec<WorkflowStep>,
    pub(crate) html_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkflowStep {
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) conclusion: String,
    #[serde(with = "rfc3339")]
    pub(crate) started_at: OffsetDateTime,
    #[serde(with = "rfc3339")]
    pub(crate) completed_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct ResponseJobs {
    jobs: Vec<WorkflowJob>,
}

// an error to convey any serde_json decoding problem.
#[derive(Debug)]
pub(crate) enum GitHubProblem {
    RemoteFailure(reqwest::Error),
    ApiError(StatusCode),
    DecodeFailure(serde_json::Error),
}

impl From<reqwest::Error> for GitHubProblem {
    fn from(error: reqwest::Error) -> Self {
        GitHubProblem::RemoteFailure(error)
    }
}

impl From<serde_json::Error> for GitHubProblem {
    fn from(error: serde_json::Error) -> Self {
        GitHubProblem::DecodeFailure(error)
    }
}

impl std::fmt::Display for GitHubProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitHubProblem::RemoteFailure(e) => write!(f, "Remote failure: {:?}", e),
            GitHubProblem::ApiError(status) => {
                write!(f, "Error response from GitHub API: {} ", status)
            }
            GitHubProblem::DecodeFailure(e) => write!(f, "Decode failure: {:?}", e),
        }
    }
}

impl std::error::Error for GitHubProblem {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitHubProblem::RemoteFailure(e) => Some(e),
            GitHubProblem::ApiError(_) => None,
            GitHubProblem::DecodeFailure(e) => Some(e),
        }
    }
}

pub(crate) async fn retrieve_run_jobs(
    config: &Config,
    client: &reqwest::Client,
    run: &WorkflowRun,
) -> Result<Vec<WorkflowJob>, GitHubProblem> {
    info!("List Jobs in Run {}", run.run_id);
    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/runs/{}/jobs",
        config.owner, config.repository, run.run_id
    );

    debug!(?url);

    let response = client
        .get(url)
        .send()
        .await?;

    // we get the whole body, then attempt to deserialize it. This allows us
    // to trap error responses coming from their API rather than just breaking
    // with decode failures. First however, we check the response code to find
    // out if we should even be trying to parse

    let status = response.status();
    let body = response
        .text()
        .await?;

    if status != StatusCode::OK {
        warn!("{}", status);
        return Err(GitHubProblem::ApiError(status));
    }

    let json: ResponseJobs = serde_json::from_str(&body)?;

    Ok(json.jobs)
}

pub(crate) async fn retrieve_job_log(
    config: &Config,
    client: &reqwest::Client,
    job_id: u64,
) -> Result<String, GitHubProblem> {
    info!("Retrieve logs for jobs {}", job_id);
    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/jobs/{}/logs",
        config.owner, config.repository, job_id
    );

    debug!(?url);

    let response = client
        .get(url)
        .send()
        .await?;

    // astonishingly, the request crate follows redirections for you by
    // default. So we don't need to worry about the 302 Found that the GitHub
    // API documentation describes at length, and instead just let the client
    // follow the redirect (and there appears to be more than one).

    let status = response.status();

    if status != StatusCode::OK {
        warn!("{}", status);

        let body = response
            .text()
            .await?;
        debug!(body);

        return Err(GitHubProblem::ApiError(status));
    }

    let body = response
        .text()
        .await?; // FIXME we need to make this streaming

    let possible = body
        .lines()
        .filter_map(|line| {
            // trim off the timestamp
            line.split_once(' ')
                .map(|(_, message)| message)
        })
        .find(|message| {
            // see if an error marker is present
            message
                .to_lowercase()
                .contains("error:")
        });
    debug!(possible);
    if let Some(message) = possible {
        Ok(message.to_string())
    } else {
        Ok(String::new())
    }
}

pub(crate) fn setup_api_client() -> Result<reqwest::Client> {
    // get GITHUB_TOKEN value passed in from environment variable
    let token = get_api_token();

    // Initialize a request Client as we will be making many requests of
    // the GitHub API.
    let mut headers = HeaderMap::new();

    // .parse() is needed here and below to get from &str to HeaderValue.

    let mut auth: HeaderValue = format!("Bearer {}", token).parse()?;
    auth.set_sensitive(true);
    headers.insert("Authorization", auth);

    headers.insert("Accept", "application/vnd.github+json".parse()?);

    headers.insert("User-Agent", format!("action-hero/{}", VERSION).parse()?);

    headers.insert("X-GitHub-Api-Version", "2022-11-28".parse()?);

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;

    Ok(client)
}
