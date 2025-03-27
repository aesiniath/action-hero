use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions::attribute::{SERVICE_NAME, SERVICE_VERSION};
use time::OffsetDateTime;
use tracing::debug;
use tracing_subscriber;

const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

mod github;
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
    // Initialize the tracing subscriber
    tracing_subscriber::fmt::init();

    // Setup OpenTelemetry. First we establish a Resource, which is a set of reusable attributes and
    // other characteristics which will be applied to all traces.

    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new(SERVICE_NAME, "github-builds"),
            KeyValue::new(SERVICE_VERSION, VERSION),
        ])
        .build();

    // Here we establish the SpanExporter subsystem that will transmit spans
    // and events out via OTLP to an otel-collector and onward to Honeycomb.

    let exporter = SpanExporter::builder()
        .with_tonic()
        .build()
        .unwrap();
    // let exporter = SpanExporter::default();

    // Now we bind this exporter and resource to a TracerProvider whose sole purpose appears to be
    // providing a way to get a Tracer which in turn is the interface used for creating spans.

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    global::set_tracer_provider(provider.clone());

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
