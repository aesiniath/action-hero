use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use time::OffsetDateTime;
use tracing::debug;
use tracing_subscriber;

const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

mod github;
mod history;
mod traces;

use github::{API, WorkflowJob, WorkflowRun};

async fn process_run(config: &API, run: &WorkflowRun) -> Result<()> {
    let context = traces::establish_root_context(&config, &run);

    let jobs: Vec<WorkflowJob> = github::retrieve_run_jobs(&config, &run).await?;

    traces::display_job_steps(&context, &run, jobs);

    traces::finalize_root_span(&context, &run);

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the logging subsystem
    tracing_subscriber::fmt::init();

    // Initialize the opentelemetry exporter
    let provider = traces::setup_telemetry_machinery();

    history::ensure_record_directory()?;

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
            .arg(
                Arg::new("devel")
                    .long("devel")
                    .long_help("Enable development mode")
                    .global(true)
                    .action(ArgAction::SetTrue))
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

    // when developing we reset all the start times to be offset from when
    // this program started running.

    let devel = *matches
        .get_one::<bool>("devel")
        .unwrap_or(&false);

    let program_start = OffsetDateTime::now_utc();

    // Now we get the details of what repository we're going to get the Action
    // history from.

    let repository = matches
        .get_one::<String>("repository")
        .unwrap()
        .to_string();

    debug!(repository);

    let (owner, repository) = repository
        .split_once('/')
        .expect("Repository must be specified in the form \"owner/repo\"");
    let owner = owner.to_owned();
    let repository = repository.to_owned();

    let workflow = matches
        .get_one::<String>("workflow")
        .unwrap()
        .to_string();

    debug!(workflow);

    let client = github::setup_api_client()?;

    let config = API {
        client,
        owner,
        repository,
        workflow,
        devel,
        program_start,
    };

    let runs: Vec<WorkflowRun> = github::retrieve_workflow_runs(&config).await?;

    // temporarily take just the first run in the list

    let run = runs
        .first()
        .unwrap();
    debug!(run.run_id);

    process_run(&config, &run).await?;

    // Ensure all spans are exported before the program exits
    provider.shutdown()?;

    Ok(())
}
