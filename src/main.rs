use anyhow::{Ok, Result};
use clap::{Arg, ArgAction, Command};
use std::sync::OnceLock;
use time::OffsetDateTime;
use tracing::{debug, info};
use tracing_subscriber;

const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

const PREFIX: &str = "record";

static PROGRAM_START: OnceLock<OffsetDateTime> = OnceLock::new();

fn set_program_start() {
    PROGRAM_START
        .set(OffsetDateTime::now_utc())
        .unwrap();
}

fn get_program_start() -> &'static OffsetDateTime {
    PROGRAM_START.wait()
}

mod github;
mod history;
mod traces;
mod webhook;

use github::{Config, WorkflowJob, WorkflowRun};

#[tokio::main]
async fn main() -> Result<()> {
    // Record start time
    set_program_start();

    // Initialize the logging subsystem
    tracing_subscriber::fmt::init();

    // Initialize the opentelemetry exporter
    let provider = traces::setup_telemetry_machinery();

    history::ensure_record_directory(PREFIX)?;

    // Configure command-line argument parser
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
            .subcommand(
                Command::new("listen")
                        .about("Run HTTP server to receive webhook events from GitHub")
                                .arg(Arg::new("port")
                                    .long("port")
                                    .long_help("Override the port the receiver will listen on. The default is port 34484")
                                )
            )
            .subcommand(
                Command::new("query")
                        .about("Query workflow runs directly")
                        .arg(
                            Arg::new("count")
                                .long("count" )
                                .long_help("The number of Runs for the specified Workflow to retrieve from GitHub and upload to Honeycomb. The default if unspecified is to check the 10 most recent Runs.")
                            )
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

            )
            .get_matches();

    // when developing we reset all the start times to be offset from when
    // this program started running.

    let devel = std::env::var("HERO_DEVELOPER").is_ok();

    match matches.subcommand() {
        Some(("listen", submatches)) => {
            let port = submatches.get_one::<String>("port");
            let port = match port {
                None => 34484,
                Some(value) => value
                    .parse::<u32>()
                    .expect("Unable to parse supplied --port value"),
            };

            let owner = String::new();
            let repository = String::new();
            let workflow = String::new();

            let config = Config {
                owner,
                repository,
                workflow,
                devel,
            };

            run_listen(&config, port).await?;
        }
        Some(("query", submatches)) => {
            // Now we get the details of what repository we're going to get the Action
            // history from.

            let repository = submatches
                .get_one::<String>("repository")
                .unwrap()
                .to_string();

            let (owner, repository) = repository
                .split_once('/')
                .expect("Repository must be specified in the form \"owner/repo\"");
            let owner = owner.to_owned();
            let repository = repository.to_owned();

            debug!(owner);
            debug!(repository);

            let workflow = submatches
                .get_one::<String>("workflow")
                .unwrap()
                .to_string();

            debug!(workflow);

            let config = Config {
                owner,
                repository,
                workflow,
                devel,
            };

            let count = submatches.get_one::<String>("count");
            let count = match count {
                None => 10,
                Some(value) => value
                    .parse::<u32>()
                    .expect("Unable to parse supplied --count value"),
            };

            run_query(&config, count).await?;
        }
        Some(_) => {
            println!("No valid subcommand was used")
        }
        None => {
            println!("usage: hero [COMMAND] ...");
            println!("Try '--help' for more information.");
        }
    }

    // Ensure all spans are exported before the program exits
    provider.shutdown()?;

    Ok(())
}

async fn run_listen(_config: &Config, port: u32) -> Result<()> {
    webhook::run_webserver(port).await
}

async fn run_query(config: &Config, count: u32) -> Result<()> {
    let client = github::setup_api_client()?;

    let runs: Vec<WorkflowRun> = github::retrieve_workflow_runs(&config, &client, count).await?;

    for run in &runs {
        let path = history::form_record_filename(PREFIX, &config, run);

        debug!(run.run_id);

        if history::check_is_submitted(&path)? {
            continue;
        }

        let trace_id = process_run(&config, &client, &run).await?;

        history::mark_run_submitted(&path, trace_id)?;
    }

    Ok(())
}

async fn process_run(
    config: &Config,
    client: &reqwest::Client,
    run: &WorkflowRun,
) -> Result<String> {
    info!("Processing Run {}", run.run_id);

    let context = traces::establish_root_context(&config, &run);

    let jobs: Vec<WorkflowJob> = github::retrieve_run_jobs(&config, client, &run).await?;

    traces::display_job_steps(&context, &run, jobs);

    let trace_id = traces::finalize_root_span(&context, &run);

    Ok(trace_id)
}
