use anyhow::Result;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::serde::rfc3339;
use tracing::{debug, info, warn};

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
    // "the start time of the latest run. Resets on re-run", as distinct from
    // created_at which stays with the attempt that has been superseded
    #[serde(with = "rfc3339")]
    pub(crate) run_started_at: OffsetDateTime,
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
            program_start - run.run_started_at - Duration::minutes(10)
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

    // and now our fields that are NOT in the response object
    #[serde(default)]
    pub(crate) uses: Option<String>,
    #[serde(default)]
    pub(crate) actions: Vec<WorkflowAction>,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// A step of a composite action. These are absent from the results returned
/// by the the main GitHub API, and instead have to be recovered by parsing
/// the raw log output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkflowAction {
    pub(crate) name: String,
    pub(crate) id: String,
    pub(crate) conclusion: String,
    pub(crate) started_at: OffsetDateTime,
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
) -> Result<Option<String>, GitHubProblem> {
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
        // logs expire after a week or less, so if trying to analyse an older
        // run the program won't be able to access detailed history.
        warn!("No log available for job {}: {}", job_id, status);

        return Ok(None);
    }

    let body = response
        .text()
        .await?; // FIXME we need to make this streaming

    Ok(Some(body))
}

fn log_lines(log: &str) -> impl Iterator<Item = (OffsetDateTime, &str)> {
    log.trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| {
            let (stamp, message) = line.split_once(' ')?;
            let stamp = OffsetDateTime::parse(stamp, &Rfc3339).ok()?;
            Some((stamp, message))
        })
}

// The runner annotates a failure with ##[error], but whatever was being run
// will have said something more specific first, and says it in its own way.
fn as_error_message(message: &str) -> Option<&str> {
    let lowered = message.to_lowercase();

    if lowered.starts_with("##[error]") || lowered.contains("error:") {
        Some(
            message
                .strip_prefix("##[error]")
                .unwrap_or(message),
        )
    } else {
        None
    }
}

struct LoggedStep {
    started_at: OffsetDateTime,
    uses: Option<String>,
    actions: Vec<WorkflowAction>,
    error: Option<String>,
}

// A step can also run a local path or a Docker image.
fn parse_uses(message: &str) -> Option<String> {
    let reference = message
        .strip_prefix("##[group]Run ")?
        .trim();

    if reference.contains('@') && !reference.contains(' ') {
        Some(reference.to_string())
    } else {
        None
    }
}

// Searched from the right; the display name can contain the separator.
fn parse_marker_field(message: &str, key: &str) -> Option<String> {
    let (_, rest) = message.rsplit_once(key)?;

    let value = match rest.split_once(';') {
        Some((value, _)) => value,
        None => rest.trim_end_matches(']'),
    };

    Some(value.to_string())
}

// Only the identifier following it marks where an author supplied name ends.
fn parse_marker_name(message: &str) -> Option<String> {
    let (_, rest) = message.split_once("display=")?;

    let (name, _) = rest.rsplit_once(";id=")?;

    Some(name.to_string())
}

// Composite steps announce themselves the same way top-level ones do, so what
// falls between the markers bracketing them belongs to the action, not the job.
fn parse_logged_steps(log: &str) -> Vec<LoggedStep> {
    let mut steps: Vec<LoggedStep> = Vec::new();
    let mut pending: Option<(String, String, OffsetDateTime)> = None;

    for (stamp, message) in log_lines(log) {
        if steps.is_empty() {
            steps.push(LoggedStep {
                started_at: stamp,
                uses: None,
                actions: Vec::new(),
                error: None,
            });
            continue;
        }

        if let Some(step) = steps.last_mut() {
            if step
                .error
                .is_none()
            {
                if let Some(text) = as_error_message(message) {
                    debug!(?text);
                    step.error = Some(text.to_string());
                }
            }
        }

        if message.starts_with("##[start-action") {
            if let Some(name) = parse_marker_name(message) {
                let id = parse_marker_field(message, "id=").unwrap_or_default();

                pending = Some((name, id, stamp));
            }
            continue;
        }

        if message.starts_with("##[end-action") {
            if let Some((name, id, started_at)) = pending.take() {
                let conclusion = parse_marker_field(message, "conclusion=").unwrap_or_default();

                if let Some(step) = steps.last_mut() {
                    step.actions
                        .push(WorkflowAction {
                            name,
                            id,
                            conclusion,
                            started_at,
                            completed_at: stamp,
                        });
                }
            }
            continue;
        }

        if pending.is_none() && message.starts_with("##[group]Run ") {
            steps.push(LoggedStep {
                started_at: stamp,
                uses: parse_uses(message),
                actions: Vec::new(),
                error: None,
            });
        }
    }

    steps
}

// The API truncates step times to whole seconds, which at least puts a step's
// true start within the second reported, and is how the two are matched here.
pub(crate) fn refine_step_times(steps: &mut [WorkflowStep], log: &str) {
    let logged = parse_logged_steps(log);

    let mut matched: Vec<(usize, LoggedStep)> = Vec::new();
    let mut cursor = 0;

    for entry in logged {
        let found = (cursor..steps.len()).find(|&index| {
            let step = &steps[index];
            let started_at = step.started_at;

            step.conclusion != "skipped"
                && entry.started_at >= started_at
                && entry.started_at - started_at < Duration::SECOND
        });

        if let Some(index) = found {
            matched.push((index, entry));
            cursor = index + 1;
        }
    }

    let starts: Vec<OffsetDateTime> = matched
        .iter()
        .map(|(_, entry)| entry.started_at)
        .collect();

    for (position, (index, entry)) in matched
        .into_iter()
        .enumerate()
    {
        let step = &mut steps[index];

        step.started_at = entry.started_at;

        step.completed_at = match starts.get(position + 1) {
            Some(&next) => next,
            None => entry
                .actions
                .last()
                .map(|action| action.completed_at)
                .unwrap_or(step.completed_at),
        };

        step.uses = entry.uses;
        step.actions = entry.actions;
        step.error = entry.error;
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

#[cfg(test)]
mod tests {
    use super::*;

    const EXCERPT: &str = concat!(
        "\u{feff}",
        r#"2026-08-07T12:01:08.8603448Z Current runner version: '2.336.0'
2026-08-07T12:01:10.5648689Z ##[group]Run actions/checkout@v6
2026-08-07T12:01:10.6839098Z ##[group]Getting Git version info
2026-08-07T12:01:11.5460245Z ##[endgroup]
2026-08-07T12:01:11.5973101Z ##[group]Run apkudo/build-image@v1
2026-08-07T12:01:11.6136875Z ##[start-action display=Determine Version;id=__apkudo_build-image.determine]
2026-08-07T12:01:11.6225529Z ##[group]Run podman build containers/internal-proxy \
2026-08-07T12:01:11.6704561Z ##[end-action id=__apkudo_build-image.determine;outcome=success;conclusion=success;duration_ms=56]
2026-08-07T12:01:11.6714578Z ##[start-action display=Tag image as :latest;id=__apkudo_build-image.tag-latest]
2026-08-07T12:02:50.0011764Z ##[end-action id=__apkudo_build-image.tag-latest;outcome=skipped;conclusion=skipped;duration_ms=0]
2026-08-07T12:02:50.0161755Z Post job cleanup.
"#
    );

    fn stamp(text: &str) -> OffsetDateTime {
        OffsetDateTime::parse(text, &Rfc3339).unwrap()
    }

    fn step(name: &str, started_at: &str, completed_at: &str) -> WorkflowStep {
        WorkflowStep {
            name: name.to_string(),
            status: "completed".to_string(),
            conclusion: "success".to_string(),
            started_at: stamp(started_at),
            completed_at: stamp(completed_at),
            uses: None,
            actions: Vec::new(),
            error: None,
        }
    }

    fn example_steps() -> Vec<WorkflowStep> {
        vec![
            step("Set up job", "2026-08-07T12:01:08Z", "2026-08-07T12:01:10Z"),
            step(
                "Checkout repository",
                "2026-08-07T12:01:10Z",
                "2026-08-07T12:01:11Z",
            ),
            step(
                "Build image",
                "2026-08-07T12:01:11Z",
                "2026-08-07T12:02:50Z",
            ),
            step(
                "Complete job",
                "2026-08-07T12:02:50Z",
                "2026-08-07T12:02:50Z",
            ),
        ]
    }

    #[test]
    fn boundaries_ignore_groups_which_are_not_steps() {
        let logged = parse_logged_steps(EXCERPT);

        let starts: Vec<OffsetDateTime> = logged
            .iter()
            .map(|entry| entry.started_at)
            .collect();

        assert_eq!(
            starts,
            vec![
                stamp("2026-08-07T12:01:08.8603448Z"),
                stamp("2026-08-07T12:01:10.5648689Z"),
                stamp("2026-08-07T12:01:11.5973101Z"),
            ]
        );
    }

    #[test]
    fn actions_are_recognised_by_their_reference() {
        let logged = parse_logged_steps(EXCERPT);

        let referenced: Vec<Option<&str>> = logged
            .iter()
            .map(|entry| {
                entry
                    .uses
                    .as_deref()
            })
            .collect();

        assert_eq!(
            referenced,
            vec![
                None,
                Some("actions/checkout@v6"),
                Some("apkudo/build-image@v1")
            ]
        );
    }

    #[test]
    fn composite_steps_belong_to_the_step_which_ran_them() {
        let mut steps = example_steps();

        refine_step_times(&mut steps, EXCERPT);

        assert!(
            steps[1]
                .actions
                .is_empty()
        );

        let actions = &steps[2].actions;

        assert_eq!(actions.len(), 2);

        assert_eq!(actions[0].name, "Determine Version");
        assert_eq!(actions[0].id, "__apkudo_build-image.determine");
        assert_eq!(actions[0].conclusion, "success");
        assert_eq!(actions[0].started_at, stamp("2026-08-07T12:01:11.6136875Z"));
        assert_eq!(
            actions[0].completed_at,
            stamp("2026-08-07T12:01:11.6704561Z")
        );

        assert_eq!(actions[1].name, "Tag image as :latest");
        assert_eq!(actions[1].conclusion, "skipped");
    }

    // ours is derived from the marker timestamps, not the reported duration
    #[test]
    fn composite_step_durations_match_the_reported_ones() {
        let mut steps = example_steps();

        refine_step_times(&mut steps, EXCERPT);

        let action = &steps[2].actions[0];

        let elapsed = action.completed_at - action.started_at;

        assert_eq!(elapsed.whole_milliseconds(), 56);
    }

    #[test]
    fn names_may_contain_a_semicolon() {
        let log = concat!(
            "2026-08-07T12:01:08.8603448Z Current runner version: '2.336.0'\n",
            "2026-08-07T12:01:11.5973101Z ##[group]Run apkudo/build-image@v1\n",
            "2026-08-07T12:01:11.6136875Z ##[start-action display=Tag; then push;id=x.y]\n",
            "2026-08-07T12:01:11.6704561Z ##[end-action id=x.y;outcome=success;conclusion=success;duration_ms=56]\n"
        );

        let logged = parse_logged_steps(log);

        let action = &logged[1].actions[0];

        assert_eq!(action.name, "Tag; then push");
        assert_eq!(action.id, "x.y");
    }

    #[test]
    fn steps_take_their_times_from_the_log() {
        let mut steps = example_steps();

        refine_step_times(&mut steps, EXCERPT);

        assert_eq!(steps[0].started_at, stamp("2026-08-07T12:01:08.8603448Z"));
        assert_eq!(steps[0].completed_at, stamp("2026-08-07T12:01:10.5648689Z"));
        assert_eq!(steps[1].started_at, stamp("2026-08-07T12:01:10.5648689Z"));
        assert_eq!(steps[1].completed_at, stamp("2026-08-07T12:01:11.5973101Z"));
        assert_eq!(steps[2].started_at, stamp("2026-08-07T12:01:11.5973101Z"));

        // no following step to end it, so its last composite step does
        assert_eq!(steps[2].completed_at, stamp("2026-08-07T12:02:50.0011764Z"));

        // never announced in the log, so reported times stand
        assert_eq!(steps[3].started_at, stamp("2026-08-07T12:02:50Z"));
        assert_eq!(steps[3].completed_at, stamp("2026-08-07T12:02:50Z"));
    }

    // A step the runner skips is still reported by the API, with a timestamp,
    // but is never announced in the log. Sharing a second with the step which
    // follows it, there is nothing but the conclusion to tell them apart.
    #[test]
    fn a_skipped_step_does_not_take_the_next_ones_times() {
        let log = concat!(
            "2026-08-07T12:01:08.8603448Z Current runner version: '2.336.0'\n",
            "2026-08-07T12:01:11.5973101Z ##[group]Run echo notifying\n"
        );

        let mut steps = vec![
            step("Set up job", "2026-08-07T12:01:08Z", "2026-08-07T12:01:11Z"),
            step("Deploy", "2026-08-07T12:01:11Z", "2026-08-07T12:01:11Z"),
            step("Notify", "2026-08-07T12:01:11Z", "2026-08-07T12:01:12Z"),
        ];
        steps[1].conclusion = "skipped".to_string();

        refine_step_times(&mut steps, log);

        assert_eq!(steps[1].started_at, stamp("2026-08-07T12:01:11Z"));
        assert_eq!(steps[2].started_at, stamp("2026-08-07T12:01:11.5973101Z"));
    }

    #[test]
    fn steps_no_longer_land_on_whole_seconds() {
        let mut steps = example_steps();

        refine_step_times(&mut steps, EXCERPT);

        let elapsed = steps[1].completed_at - steps[1].started_at;

        assert!(elapsed > Duration::SECOND);
        assert_eq!(elapsed, Duration::nanoseconds(1_032_441_200));
    }

    #[test]
    fn absent_markers_leave_the_steps_alone() {
        let mut steps = example_steps();

        refine_step_times(&mut steps, "");

        for (refined, original) in steps
            .iter()
            .zip(example_steps())
        {
            assert_eq!(refined.started_at, original.started_at);
            assert_eq!(refined.completed_at, original.completed_at);
        }
    }

    // A re-run keeps the created_at of the attempt it replaced, so the Run
    // would otherwise appear to have begun long before any of its Jobs.
    #[test]
    fn a_re_run_begins_when_the_latest_attempt_did() {
        let body = r#"{
            "actor": { "login": "aesiniath" },
            "id": 31173393911,
            "run_number": 42,
            "run_attempt": 2,
            "head_branch": "main",
            "name": "Build",
            "display_title": "Build",
            "event": "pull_request",
            "status": "completed",
            "conclusion": "success",
            "created_at": "2026-08-07T11:16:12Z",
            "run_started_at": "2026-08-07T12:01:01Z",
            "updated_at": "2026-08-07T12:02:53Z",
            "html_url": "https://example.com",
            "path": ".github/workflows/build.yaml"
        }"#;

        let run: WorkflowRun = serde_json::from_str(body).unwrap();

        assert_eq!(run.run_started_at, stamp("2026-08-07T12:01:01Z"));
    }

    #[test]
    fn error_message_is_found_without_its_timestamp() {
        assert_eq!(
            as_error_message("##[error]Error: getaddrinfo ENOTFOUND"),
            Some("Error: getaddrinfo ENOTFOUND")
        );
    }

    // The runner's own annotations carry no colon, and were not matched at all.
    #[test]
    fn an_annotation_is_an_error() {
        assert_eq!(
            as_error_message("##[error]Unable to resolve actions."),
            Some("Unable to resolve actions.")
        );

        assert_eq!(as_error_message("Getting image source signatures"), None);
    }

    // Taking the first leaves the runner's generic trailer behind in favour of
    // whatever the failing command had to say for itself.
    #[test]
    fn a_specific_message_beats_the_annotation_which_follows_it() {
        let log = concat!(
            "2026-08-07T11:16:14.0000000Z Current runner version: '2.336.0'\n",
            "2026-08-07T11:16:15.0000000Z ##[group]Run podman build .\n",
            "2026-08-07T11:16:16.0000000Z error: Linting: Failed to parse sysusers entry\n",
            "2026-08-07T11:16:17.0000000Z Error: building at STEP \"RUN\": exit status 1\n",
            "2026-08-07T11:16:18.0000000Z ##[error]Process completed with exit code 1.\n"
        );

        let logged = parse_logged_steps(log);

        assert_eq!(
            logged[1]
                .error
                .as_deref(),
            Some("error: Linting: Failed to parse sysusers entry")
        );
    }

    #[test]
    fn error_belongs_to_producing_step() {
        let log = concat!(
            "2026-08-07T11:16:14.0000000Z Current runner version: '2.336.0'\n",
            "2026-08-07T11:16:15.0000000Z ##[group]Run ./flaky.sh\n",
            "2026-08-07T11:16:16.0000000Z error: transient, retrying\n",
            "2026-08-07T11:16:17.0000000Z ##[group]Run ./deploy.sh\n",
            "2026-08-07T11:16:18.0000000Z ##[error]deploy rejected by remote\n"
        );

        let logged = parse_logged_steps(log);

        assert_eq!(logged[0].error, None);
        assert_eq!(
            logged[1]
                .error
                .as_deref(),
            Some("error: transient, retrying")
        );
        assert_eq!(
            logged[2]
                .error
                .as_deref(),
            Some("deploy rejected by remote")
        );
    }

    // A failure before any step is announced belongs to the setup the runner
    // was doing at the time, which is a step in its own right.
    #[test]
    fn failure_during_setup_belongs_to_first_step() {
        let log = concat!(
            "2026-08-07T11:16:14.0000000Z Current runner version: '2.336.0'\n",
            "2026-08-07T11:16:16.3532464Z ##[error]Unable to resolve actions.\n"
        );

        let logged = parse_logged_steps(log);

        assert_eq!(logged.len(), 1);
        assert_eq!(
            logged[0]
                .error
                .as_deref(),
            Some("Unable to resolve actions.")
        );
    }
}
