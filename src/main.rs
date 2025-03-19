use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use serde_json::Value;
use tracing::{debug, info};
use tracing_subscriber;

const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

async fn retrieve_workflow_runs(
    client: &reqwest::Client,
    owner: &str,
    repository: &str,
    workflow: &str,
) -> Result<Vec<String>> {
    // get GITHUB_TOKEN value from environment variable
    let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN environment variable not set");

    // use token to retrieve runs for the given workflow from GitHub API

    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/workflows/{}/runs?per_page=10&page=1",
        owner, repository, workflow
    );
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", format!("action-hero/{}", VERSION))
        .send()
        .await?;

    // retrieve the run ID of the most recent 10 runs
    let body: Value = response
        .json()
        .await?;

    let runs: Vec<String> = body["workflow_runs"]
        .as_array()
        .expect("Expected workflow_runs to be an array")
        .iter()
        .take(10)
        .map(|workflow_run| {
            workflow_run["id"]
                .as_i64()
                .expect("Expected run ID to be present and non-empty")
                .to_string()
        })
        .collect();

    Ok(runs)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the tracing subscriber
    tracing_subscriber::fmt::init();

    let matches = Command::new("hero")
            .version(VERSION)
            .propagate_version(true)
            .author("Andrew Cowie")
            .about("Retrieve workflow and run from GitHub Actions and send to OpenTelemetry as spans and traces.")
            .disable_help_subcommand(true)
            .disable_help_flag(true)
            .disable_version_flag(true)
            .arg(
                Arg::new("help")
                    .long("help")
                    .long_help("Print help")
                    .global(true)
                    .hide(true)
                    .action(ArgAction::Help))
            .arg(
                Arg::new("version")
                    .long("version")
                    .long_help("Print version")
                    .global(true)
                    .hide(true)
                    .action(ArgAction::Version))
            .arg(
                Arg::new("repository")
                    .action(ArgAction::Set)
                    .required(true)
                    .help("Name of the GitHub organization and repository to retrieve workflows from. This must be specified in the form \"owner/repo\""))
            .arg(
                Arg::new("workflow")
                    .action(ArgAction::Set)
                    .required(true)
                    .help("Name of the GitHub Actions workflow to present as a trace. This is typically a filename such as \"check.yaml\""))
            .get_matches();

    let repository = matches
        .get_one::<String>("repository")
        .unwrap()
        .to_string();

    debug!(repository);

    let (owner, repository) = repository
        .split_once('/')
        .expect("Repository must be specified in the form \"owner/repo\"");

    let workflow = matches
        .get_one::<String>("workflow")
        .unwrap()
        .to_string();

    debug!(workflow);

    // Initialize a request Client as we will be making many requests of
    // the GitHub API.
    let client = reqwest::Client::new();

    let runs: Vec<String> = retrieve_workflow_runs(&client, &owner, &repository, &workflow).await?;

    println!("runs: {:#?}", runs);
    Ok(())
}
