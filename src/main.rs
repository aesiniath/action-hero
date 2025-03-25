use anyhow::Result;
use clap::{Arg, ArgAction, Command};
use opentelemetry::trace::{
    Span, SpanBuilder, SpanContext, TraceContextExt, TraceState, TracerProvider,
};
use opentelemetry::{Context, KeyValue, SpanId, TraceFlags, TraceId, global, trace::Tracer};
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions::attribute::{SERVICE_NAME, SERVICE_VERSION};
use std::process;
use time::Duration;
// use opentelemetry_stdout::SpanExporter;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;
use sha2::Digest;
use std::time::SystemTime;
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
    devel: bool,
    program_start: OffsetDateTime,
}

/// It turns out that the OpenTelemetry API uses std::time::SystemTime to
/// represent start and end times (which makes sense, given that is mostly
/// about getting now() from the OS, but they are otherwise a little difficult
/// to construct). This function converts from the OffsetDateTime produced by
/// the *time* crate's parser to SystemTime.
fn convert_to_system_time(datetime: &OffsetDateTime) -> SystemTime {
    datetime
        .to_offset(time::UtcOffset::UTC)
        .into()
}

fn form_trace_id(config: &API, run_id: &str) -> TraceId {
    let input = format!(
        "{}:{}:{}:{}",
        config.owner, config.repository, config.workflow, run_id
    );

    let mut hasher = sha2::Sha256::new();
    hasher.update(input.as_bytes());

    // if we signal that we're doing development we mix in the PID to override
    // the otherwise deterministic nature of assigning a TraceID so we can get
    // separate traces into Honeycomb when testing.

    if config.devel {
        let pid = process::id();
        hasher.update(pid.to_le_bytes());
    }

    let result = hasher.finalize();

    // Trace IDs are defined as being 128 bits, so somewhat arbitrarily we
    // just select half of the 256 bit hash result.

    let lower: [u8; 16] = result[..16]
        .try_into()
        .unwrap();

    TraceId::from_bytes(lower)
}

#[derive(Debug)]
struct WorkflowRun {
    run_id: String,
    name: String,
    status: String,
    conclusion: String,
    created_at: SystemTime,
    delta: Duration,
}

async fn retrieve_workflow_runs(config: &API) -> Result<Vec<WorkflowRun>> {
    // use token to retrieve runs for the given workflow from GitHub API

    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/workflows/{}/runs?per_page=10&page=1",
        config.owner, config.repository, config.workflow
    );
    debug!(?url);

    let response = config
        .client
        .get(&url)
        .send()
        .await?;

    // retrieve the run ID of the most recent 10 runs
    let body: Value = response
        .json()
        .await?;

    let runs: Vec<WorkflowRun> = body["workflow_runs"]
        .as_array()
        .expect("Expected workflow_runs to be an array")
        .iter()
        .take(10)
        .map(|workflow_run| {
            let run_id = workflow_run["id"]
                .as_i64()
                .expect("Expected run ID to be present and non-empty")
                .to_string();

            let name = workflow_run["name"]
                .as_str()
                .expect("Expected run name to be present and non-empty")
                .to_string();

            let status = workflow_run["status"]
                .as_str()
                .expect("Expected run status to be present and non-empty")
                .to_string();

            let conclusion = workflow_run["conclusion"]
                .as_str()
                .expect("Expected run conclusion to be present and non-empty")
                .to_string();

            let created_at = workflow_run["created_at"]
                .as_str()
                .expect("Expected run created_at to be present and non-empty")
                .to_string();
            let created_at = OffsetDateTime::parse(&created_at, &Rfc3339).unwrap();

            // calculate the change to the origin time if we are in development
            // mode. This delta will be added to all timestamps to bring them to
            // near program start time (ie now).
            let delta = if config.devel {
                config.program_start - created_at
            } else {
                Duration::ZERO
            };
            let created_at = created_at + delta;

            let created_at = convert_to_system_time(&created_at);

            WorkflowRun {
                run_id,
                name,
                status,
                conclusion,
                created_at,
                delta,
            }
        })
        .collect();

    Ok(runs)
}

async fn retrieve_run_jobs(config: &API, run: &WorkflowRun) -> Result<Vec<Value>> {
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
        .json::<serde_json::Value>()
        .await?;

    let jobs: Vec<Value> = body["jobs"]
        .as_array()
        .expect("Expected jobs to be an array")
        .to_vec();

    Ok(jobs)
}

// returns the earliest start and latest finishing time of jobs seen within
// the run, so the root span can be updated accordingly. We originally had
// "context" named "parent" was a somewhat misleading name; it is the current
// Context _containing_ a span and as such will become the parent.
fn display_job_steps(
    context: &Context,
    run: &WorkflowRun,
    jobs: &Vec<serde_json::Value>,
) -> (SystemTime, SystemTime) {
    let mut earliest_start = SystemTime::now();
    let mut latest_finish = SystemTime::UNIX_EPOCH;

    let provider = global::tracer_provider();
    let tracer = provider.tracer(module_path!());

    for job in jobs {
        let job_name = job["name"]
            .as_str()
            .unwrap();

        println!("{}", job_name);

        let steps = job["steps"]
            .as_array()
            .expect("Expected steps to be an array");

        // get job start and end times
        let job_start = job["started_at"]
            .as_str()
            .unwrap();
        let job_start = OffsetDateTime::parse(job_start, &Rfc3339).unwrap();

        let job_finish = job["completed_at"]
            .as_str()
            .unwrap();
        let job_finish = OffsetDateTime::parse(job_finish, &Rfc3339).unwrap();

        let job_start = job_start + run.delta;
        let job_finish = job_finish + run.delta;

        let job_start = convert_to_system_time(&job_start);
        let job_finish = convert_to_system_time(&job_finish);

        // setup a new child span
        let builder = SpanBuilder::from_name(job_name.to_owned())
            .with_start_time(job_start)
            .with_end_time(job_finish);

        let span = tracer.build_with_context(builder, &context);

        // and again non-obviously, although the Job span is now a child, the
        // context still has the root span in it. We need to get a new context
        // before creating spans around the Steps.
        let context = context.with_span(span);
        // and stupidly, get it out again
        let span = context.span();

        let job_conclusion = job["conclusion"]
            .as_str()
            .unwrap();
        span.set_attribute(KeyValue::new("conclusion", job_conclusion.to_owned()));

        let head_branch = job["head_branch"]
            .as_str()
            .unwrap();
        span.set_attribute(KeyValue::new("head_branch", head_branch.to_owned()));

        // now iterate through the steps of this job, and extract the details
        // to be put onto individual grandchild spans.
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

            // convert start and stop times to a suitable DateTime type. We
            // add "delta" to reset the origin to the program start time if
            // doing development.

            let step_start = OffsetDateTime::parse(step_start, &Rfc3339).unwrap() + run.delta;
            let step_finish = OffsetDateTime::parse(step_finish, &Rfc3339).unwrap() + run.delta;

            let step_duration = step_finish - step_start;

            println!("    {}: {}, {}", step_name, step_status, step_duration);

            // Get read to send OpenTelemetry data

            // And now at last we create a span. It's not clear if setting the
            // end time does any good here, as we have to close a span with a
            // timestamp (otherwise it gets told to be now() from a few
            // places)

            let step_start = convert_to_system_time(&step_start);
            let step_finish = convert_to_system_time(&step_finish);

            let builder = SpanBuilder::from_name(step_name.to_owned())
                .with_start_time(step_start)
                .with_end_time(step_finish);

            // because context has a current Span present within it this
            // will create the new Span as a child of that one as parent!
            let mut span = tracer.build_with_context(builder, &context);
            span.set_attribute(KeyValue::new("step.status", step_status.to_owned()));

            span.end_with_timestamp(step_finish);
        }

        // finalize the enclosing job span and send. We kept this in scope
        // while the spans were created around individual steps so they would
        // be children of this job's span.
        span.end_with_timestamp(job_finish);

        if job_start < earliest_start {
            earliest_start = job_start;
        }
        if job_finish > latest_finish {
            latest_finish = job_finish;
        }
    }

    (earliest_start, latest_finish)
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

fn establish_root_context(config: &API, run: &WorkflowRun) -> Context {
    let provider = global::tracer_provider();
    let tracer = provider.tracer(module_path!());

    let trace_id = form_trace_id(&config, &run.run_id);

    // this is meant to be the immutable, reusable part of a trace that can be
    // propagated to a remote process (or received from a invoking parent). In our
    // case we just need to control the TraceId value being used.
    let span_context = SpanContext::new(
        trace_id,
        SpanId::INVALID,
        TraceFlags::SAMPLED,
        false,
        TraceState::NONE,
    );

    // the naming of this is odd, and the fact that it's hidden on TraceContextExt is
    // unhelpful to say the least.
    let context = Context::new().with_remote_span_context(span_context);

    let builder = SpanBuilder::from_name(
        run.name
            .to_owned(),
    )
    .with_start_time(run.created_at);

    let span = tracer.build_with_context(builder, &context);

    // more non-obvious: set the span into the Context,
    let context = context.with_span(span);

    // and return it
    context
}

fn finalize_root_span(context: &Context, earliest_start: SystemTime, latest_finish: SystemTime) {
    let span = context.span();
    let span_context = span.span_context();
    let trace_id = span_context.trace_id();
    let span_id = span_context.span_id();

    debug!(?span_id);
    debug!(?trace_id);

    // this SHOULD be the root span!
    span.set_attribute(KeyValue::new("debug.omega", true));
    span.end_with_timestamp(latest_finish);
}

async fn process_run(config: &API, run: &WorkflowRun) -> Result<()> {
    let context = establish_root_context(&config, &run);

    let jobs: Vec<Value> = retrieve_run_jobs(&config, &run).await?;

    let (earliest, latest) = display_job_steps(&context, &run, &jobs);

    finalize_root_span(&context, earliest, latest);

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

    let client = setup_api_client()?;

    let config = API {
        client,
        owner,
        repository,
        workflow,
        devel,
        program_start,
    };

    let runs: Vec<WorkflowRun> = retrieve_workflow_runs(&config).await?;

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
