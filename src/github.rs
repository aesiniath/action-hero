use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use time::Duration;
use time::OffsetDateTime;
use time::serde::rfc3339;
use tracing::debug;
use tracing::info;

use crate::VERSION;

/// A struct holding the configuration being used to retrieve information from
/// GitHub's API.
pub(crate) struct API {
    pub(crate) client: reqwest::Client,
    pub(crate) owner: String,
    pub(crate) repository: String,
    pub(crate) workflow: String,
    pub(crate) devel: bool,
    pub(crate) program_start: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowRun {
    #[serde(rename = "id")]
    pub(crate) run_id: u64,
    pub(crate) run_number: u64,
    pub(crate) run_attempt: u64,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) conclusion: String,
    #[serde(with = "rfc3339")]
    pub(crate) created_at: OffsetDateTime,
    #[serde(with = "rfc3339")]
    pub(crate) updated_at: OffsetDateTime,
    pub(crate) html_url: String,
    // and now our fields that are NOT in the response object
    #[serde(default)]
    pub(crate) delta: Duration,
}

#[derive(Deserialize)]
struct ResponseRuns {
    workflow_runs: Vec<WorkflowRun>,
}

pub(crate) async fn retrieve_workflow_runs(config: &API, count: u32) -> Result<Vec<WorkflowRun>> {
    // use token to retrieve runs for the given workflow from GitHub API
    info!("List Runs for Workflow {}", config.workflow);

    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/workflows/{}/runs?per_page={}&page=1",
        config.owner, config.repository, config.workflow, count
    );
    debug!(?url);

    let response = config
        .client
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
            config.program_start - run.created_at - Duration::minutes(10)
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

pub(crate) async fn retrieve_run_jobs(config: &API, run: &WorkflowRun) -> Result<Vec<WorkflowJob>> {
    info!("List Jobs in Run {}", run.run_id);
    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/runs/{}/jobs",
        config.owner, config.repository, run.run_id
    );

    debug!(?url);

    let response = config
        .client
        .get(url)
        .send()
        .await?;

    let body = response
        .json::<ResponseJobs>()
        .await?;

    Ok(body.jobs)
}

pub(crate) fn setup_api_client() -> Result<reqwest::Client> {
    // get GITHUB_TOKEN value from environment variable
    let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN environment variable not set");

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
