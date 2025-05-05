use opentelemetry::trace::{
    Span, SpanBuilder, SpanContext, TraceContextExt, TraceState, TracerProvider,
};
use opentelemetry::{Context, KeyValue, SpanId, TraceFlags, TraceId, global, trace::Tracer};
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions::attribute::{SERVICE_NAME, SERVICE_VERSION};
use std::borrow::Cow;
use std::process;
// use opentelemetry_stdout::SpanExporter;
use sha2::Digest;
use std::time::SystemTime;
use time::OffsetDateTime;
use tracing::debug;

use crate::VERSION;
use crate::github::{Config, GitHubProblem, WorkflowJob, WorkflowRun, retrieve_job_log};

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

fn form_trace_id(config: &Config, run_id: u64) -> TraceId {
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

// returns the earliest start and latest finishing time of jobs seen within
// the run, so the root span can be updated accordingly. We originally had
// "context" named "parent" was a somewhat misleading name; it is the current
// Context _containing_ a span and as such will become the parent.
pub(crate) async fn display_job_steps(
    config: &Config,
    client: &reqwest::Client,
    context: &Context,
    run: &WorkflowRun,
    jobs: Vec<WorkflowJob>,
) -> Result<(), GitHubProblem> {
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

        span.set_attribute(KeyValue::new("layer", "Job"));

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

            span.set_attribute(KeyValue::new("layer", "Step"));

            span.set_attribute(KeyValue::new("status", step.status));

            if step.conclusion == "failure" {
                span.set_status(opentelemetry::trace::Status::Error {
                    description: Cow::Borrowed("Step failed"),
                });

                let message = retrieve_job_log(config, client, job.job_id).await?;
                span.set_attribute(KeyValue::new("exception.message", message));
            }
            span.set_attribute(KeyValue::new("conclusion", step.conclusion));

            span.end_with_timestamp(step_finish);
        }

        // finalize the enclosing job span and send. We kept this in scope
        // while the spans were created around individual steps so they would
        // be children of this job's span.
        span.end_with_timestamp(job_finish);
    }

    Ok(())
}

pub(crate) fn establish_root_context(config: &Config, run: &WorkflowRun) -> Context {
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

    span.set_attribute(KeyValue::new("layer", "Run"));

    span.set_attribute(KeyValue::new("owner", owner));

    span.set_attribute(KeyValue::new("repository", repository));

    span.set_attribute(KeyValue::new("workflow", workflow));

    span.set_attribute(KeyValue::new("run_id", run.run_id as i64));

    if let Some(value) = conclusion {
        span.set_attribute(KeyValue::new("conclusion", value));
    }
    span.set_attribute(KeyValue::new("status", status));

    span.set_attribute(KeyValue::new("html_url", html_url));

    span.set_attribute(KeyValue::new("run_number", run_number));

    span.set_attribute(KeyValue::new("run_attempt", run_attempt));

    // more non-obvious: set the span into the Context,
    let context = context.with_span(span);

    // and return it
    context
}

pub(crate) fn finalize_root_span(context: &Context, run: &WorkflowRun) -> String {
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

    format!("{:x}", trace_id)
}

pub(crate) fn setup_telemetry_machinery() -> SdkTracerProvider {
    // Setup OpenTelemetry. First we establish a Resource, which is a set of reusable attributes and
    // other characteristics which will be applied to all traces.

    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new(SERVICE_NAME, "github-actions"),
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

    provider
}
