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
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::time::SystemTime;
use time::OffsetDateTime;
use time::serde::rfc3339;
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

fn form_trace_id(config: &API, run_id: u64) -> TraceId {
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

#[derive(Debug, Deserialize)]
struct WorkflowRun {
    #[serde(rename = "id")]
    run_id: u64,
    run_number: u64,
    run_attempt: u64,
    name: String,
    status: String,
    conclusion: String,
    #[serde(with = "rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "rfc3339")]
    updated_at: OffsetDateTime,
    html_url: String,
    // and now our fields that are NOT in the response object
    #[serde(default)]
    delta: Duration,
}

#[derive(Deserialize)]
struct ResponseRuns {
    workflow_runs: Vec<WorkflowRun>,
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
    let body: ResponseRuns = response
        .json()
        .await?;

    let mut runs: Vec<WorkflowRun> = body.workflow_runs;

    for run in runs
        .iter_mut()
        .take(10)
    {
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
struct WorkflowJob {
    #[serde(rename = "id")]
    job_id: u64,
    name: String,
    head_branch: String,
    status: String,
    conclusion: String,
    #[serde(with = "rfc3339")]
    started_at: OffsetDateTime,
    #[serde(with = "rfc3339")]
    completed_at: OffsetDateTime,
    steps: Vec<WorkflowStep>,
    html_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkflowStep {
    name: String,
    status: String,
    conclusion: String,
    #[serde(with = "rfc3339")]
    started_at: OffsetDateTime,
    #[serde(with = "rfc3339")]
    completed_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct ResponseJobs {
    jobs: Vec<WorkflowJob>,
}

async fn retrieve_run_jobs(config: &API, run: &WorkflowRun) -> Result<Vec<WorkflowJob>> {
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

// returns the earliest start and latest finishing time of jobs seen within
// the run, so the root span can be updated accordingly. We originally had
// "context" named "parent" was a somewhat misleading name; it is the current
// Context _containing_ a span and as such will become the parent.
fn display_job_steps(
    context: &Context,
    run: &WorkflowRun,
    jobs: Vec<WorkflowJob>,
) -> (SystemTime, SystemTime) {
    let mut earliest_start = SystemTime::now();
    let mut latest_finish = SystemTime::UNIX_EPOCH;

    let provider = global::tracer_provider();
    let tracer = provider.tracer(module_path!());

    for job in jobs {
        println!("{}", job.name);

        // get job start and end times
        let job_start = job.started_at + run.delta;
        let job_finish = job.completed_at + run.delta;

        let job_start = convert_to_system_time(&job_start);
        let job_finish = convert_to_system_time(&job_finish);

        // setup a new child span
        let builder = SpanBuilder::from_name(job.name)
            .with_start_time(job_start)
            .with_end_time(job_finish);

        let span = tracer.build_with_context(builder, &context);

        // and again non-obviously, although the Job span is now a child, the
        // context still has the root span in it. We need to get a new context
        // before creating spans around the Steps.
        let context = context.with_span(span);
        // and stupidly, get it out again
        let span = context.span();

        span.set_attribute(KeyValue::new("job_id", job.job_id as i64));

        span.set_attribute(KeyValue::new("conclusion", job.conclusion));

        span.set_attribute(KeyValue::new("status", job.status));

        span.set_attribute(KeyValue::new("head_branch", job.head_branch));

        span.set_attribute(KeyValue::new("html_url", job.html_url));

        // now iterate through the steps of this job, and extract the details
        // to be put onto individual grandchild spans.
        for step in job.steps {
            // convert start and stop times to a suitable DateTime type. We
            // add "delta" to reset the origin to the program start time if
            // doing development.

            let step_start = step.started_at + run.delta;
            let step_finish = step.completed_at + run.delta;

            let step_duration = step_finish - step_start;

            println!("    {}: {}, {}", step.name, step.status, step_duration);

            // Get read to send OpenTelemetry data

            // And now at last we create a span. It's not clear if setting the
            // end time does any good here, as we have to close a span with a
            // timestamp (otherwise it gets told to be now() from a few
            // places)

            let step_start = convert_to_system_time(&step_start);
            let step_finish = convert_to_system_time(&step_finish);

            let builder = SpanBuilder::from_name(step.name)
                .with_start_time(step_start)
                .with_end_time(step_finish);

            // because context has a current Span present within it this
            // will create the new Span as a child of that one as parent!
            let mut span = tracer.build_with_context(builder, &context);
            span.set_attribute(KeyValue::new("status", step.status));

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

fn establish_root_context(config: &API, run: &WorkflowRun) -> Context {
    let provider = global::tracer_provider();
    let tracer = provider.tracer(module_path!());

    let trace_id = form_trace_id(&config, run.run_id);

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

    let name = run
        .name
        .clone();
    let owner = config
        .owner
        .clone();
    let repository = config
        .repository
        .clone();
    let workflow = config
        .workflow
        .clone();
    let conclusion = run
        .conclusion
        .clone();
    let status = run
        .status
        .clone();
    let html_url = run
        .html_url
        .clone();
    let run_number = run.run_number as i64;
    let run_attempt = run.run_attempt as i64;

    // adjust the span start time if we are in development mode
    let created_at = run.created_at + run.delta;
    let run_start = convert_to_system_time(&created_at);

    // the naming of this is odd, and the fact that it's hidden on TraceContextExt is
    // unhelpful to say the least.
    let context = Context::new().with_remote_span_context(span_context);

    let builder = SpanBuilder::from_name(name).with_start_time(run_start);

    // create the span that will be the root span
    let mut span = tracer.build_with_context(builder, &context);

    span.set_attribute(KeyValue::new("owner", owner));

    span.set_attribute(KeyValue::new("repository", repository));

    span.set_attribute(KeyValue::new("workflow", workflow));

    span.set_attribute(KeyValue::new("run_id", run.run_id as i64));

    span.set_attribute(KeyValue::new("conclusion", conclusion));

    span.set_attribute(KeyValue::new("status", status));

    span.set_attribute(KeyValue::new("html_url", html_url));

    span.set_attribute(KeyValue::new("run_number", run_number));

    span.set_attribute(KeyValue::new("run_attempt", run_attempt));

    // more non-obvious: set the span into the Context,
    let context = context.with_span(span);

    // and return it
    context
}

fn finalize_root_span(context: &Context, run: &WorkflowRun) {
    let span = context.span();
    let span_context = span.span_context();
    let trace_id = span_context.trace_id();
    let span_id = span_context.span_id();

    let run_finish = run.updated_at + run.delta;
    let run_finish = convert_to_system_time(&run_finish);
    debug!(?span_id);
    debug!(?trace_id);

    // this SHOULD be the root span!
    span.set_attribute(KeyValue::new("debug.omega", true));
    span.end_with_timestamp(run_finish);
}

async fn process_run(config: &API, run: &WorkflowRun) -> Result<()> {
    let context = establish_root_context(&config, &run);

    let jobs: Vec<WorkflowJob> = retrieve_run_jobs(&config, &run).await?;

    let (earliest, latest) = display_job_steps(&context, &run, jobs);

    finalize_root_span(&context, &run);

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
