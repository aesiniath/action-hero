use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use opentelemetry::trace::{Span, SpanBuilder, TracerProvider};
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions::attribute::{SERVICE_NAME, SERVICE_VERSION};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;
use std::time::{Duration, SystemTime};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::debug;
use tracing_subscriber;

const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

/// A struct holding the configuration being used to retrieve information from
/// GitHub's API.
struct API {
    client: reqwest::Client,
    owner: String,
    repository: String,
    workflow: String,
}

/// It turns out that the OpenTelemetry API uses std::time::SystemTime to
/// represent start and end times (which makes sense, given that is mostly
/// about getting now() from the OS, but they are otherwise a little difficult
/// to construct). This function converts from the OffsetDateTime produced by
/// the *time* crate's parser to SystemTime.
fn convert_to_system_time(datetime: &OffsetDateTime) -> SystemTime {
    let unix_timestamp = datetime.unix_timestamp();
    SystemTime::UNIX_EPOCH + Duration::from_secs(unix_timestamp as u64)
}

async fn retrieve_workflow_runs(api: &API) -> Result<Vec<String>> {
    // use token to retrieve runs for the given workflow from GitHub API

    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/workflows/{}/runs?per_page=10&page=1",
        api.owner, api.repository, api.workflow
    );
    debug!(?url);

    let response = api
        .client
        .get(&url)
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

async fn retrieve_run_jobs(api: &API, run_id: &str) -> Result<Vec<Value>> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/runs/{}/jobs",
        api.owner, api.repository, run_id
    );

    debug!(?url);

    let response = api
        .client
        .get(url)
        .send()
        .await?;

    let body = response
        .json::<serde_json::Value>()
        .await?;

    let jobs: Vec<Value> = body["jobs"]
        .as_array()
        .expect("Expected jobs to be an array")
        .to_vec();

    Ok(jobs)
}

fn display_job_steps(jobs: &Vec<serde_json::Value>) {
    for job in jobs {
        let job_name = job["name"]
            .as_str()
            .unwrap();

        println!("{}", job_name);

        let steps = job["steps"]
            .as_array()
            .expect("Expected steps to be an array");

        for step in steps {
            let step_name = step["name"]
                .as_str()
                .unwrap();
            let step_status = step["status"]
                .as_str()
                .unwrap();
            let step_start = step["started_at"]
                .as_str()
                .unwrap();
            let step_finish = step["completed_at"]
                .as_str()
                .unwrap();

            // convert start and stop times to a suitable DateTime type

            let step_start = OffsetDateTime::parse(step_start, &Rfc3339).unwrap();
            let step_finish = OffsetDateTime::parse(step_finish, &Rfc3339).unwrap();

            let step_duration = step_finish - step_start;

            println!("    {}: {}, {}", step_name, step_status, step_duration);

            // Get read to send OpenTelemetry data

            let provider = global::tracer_provider();
            let tracer = provider.tracer("fixme-1");

            // It's not clear if setting the end time does any good here, as
            // we have to close a span with a timestamp (otherwise it gets
            // told to be now() from a few places)

            let step_start = convert_to_system_time(&step_start);
            let step_finish = convert_to_system_time(&step_finish);

            let mut span = SpanBuilder::from_name(step_name)
                .with_start_time(step_start)
                .with_end_time(step_finish)
                .start(&tracer);

            span.set_attribute(KeyValue::new("step.status", step_status));

            span.end_with_timestamp(step_finish);
        }
    }
}

fn setup_api_client() -> Result<reqwest::Client> {
    // get GITHUB_TOKEN value from environment variable
    let token = std::env::var("GITHUB_TOKEN").expect("GITHUB_TOKEN environment variable not set");

    // Initialize a request Client as we will be making many requests of
    // the GitHub API.
    let mut headers = HeaderMap::new();

    let mut auth: HeaderValue = format!("Bearer {}", token)
        .parse()
        .unwrap();
    auth.set_sensitive(true);
    headers.insert("Authorization", auth);

    headers.insert(
        "Accept",
        "application/vnd.github+json"
            .parse()
            .unwrap(),
    );

    headers.insert(
        "User-Agent",
        format!("action-hero/{}", VERSION)
            .parse()
            .unwrap(),
    );

    headers.insert(
        "X-GitHub-Api-Version",
        "2022-11-28"
            .parse()
            .unwrap(),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;

    Ok(client)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the tracing subscriber
    tracing_subscriber::fmt::init();

    // Setup OpenTelemetry. First we establish a Resource, which is a set of reusable attributes and
    // other characteristics which will be applied to all traces.

    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new(SERVICE_NAME, env!("CARGO_PKG_NAME")),
            KeyValue::new(SERVICE_VERSION, VERSION),
        ])
        .build();

    // Here we establish the SpanExporter subsystem that will transmit spans
    // and events out via OTLP to an otel-collector and onwards to Honeycomb.

    let exporter = SpanExporter::builder()
        .with_tonic()
        .build()
        .unwrap();

    // Now we bind this exporter and resource to a TracerProvider whose sole purpose appears to be
    // providing a way to get a Tracer which in turn is the interface used for creating spans.

    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter)
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
    let owner = owner.to_owned();
    let repository = repository.to_owned();

    let workflow = matches
        .get_one::<String>("workflow")
        .unwrap()
        .to_string();

    debug!(workflow);

    let client = setup_api_client()?;

    let api = API {
        client,
        owner,
        repository,
        workflow,
    };

    let runs: Vec<String> = retrieve_workflow_runs(&api).await?;

    println!("runs: {:#?}", runs);

    let run_id: &str = runs
        .first()
        .unwrap()
        .as_ref();

    debug!(run_id);

    let jobs: Vec<Value> = retrieve_run_jobs(&api, &run_id).await?;

    display_job_steps(&jobs);

    // Ensure all spans are exported before the program exits
    provider.shutdown()?;

    Ok(())
}
