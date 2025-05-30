# Action Hero

_When you're doing Actions, the only thing that will save you is a Hero!_

![Two superheros in a datacenter weaving magical spells](doc/ActionHerosInDatacenter.jpg)

## Background

We wanted to get traces of our CI builds into Honeycomb.

Wanting to instrument build pipelines isn't especially new, but most of the
solutions out there work by having you edit the `run:` clauses of your steps
to invoke a wrapper program.

That works, but it's terribly invasive. You have to change _every_ workflow
file to invoke the wrapper script, and configure it with secrets, on it goes.
Even if you're willing to put up with all that, it's still not good enough,
because you can't put the wrapper in front of actions that _don't_ use a
`run:` block. So there are tonnes of steps you just wouldn't get any telemetry
about at all, taking time. Sneakily. You know, just the sort of thing a comic
book villain would trick you into doing to yourself.

The solution adopted by **action-hero** is to invoke the GitHub API and then
convert the response into traces and spans after the fact. For a given
Repository and Workflow, this program requests a list of Runs, then requests
the history of the Jobs that run and the steps in each Job. It composes them
into spans with appropriate start and finish times, then sends it through your
local OpenTelemetry Collector up off to Honeycomb.

![Example Trace](doc/TraceExample.png)

## Usage

You pass the name of the repository (qualified with the owner or organization
it belongs to) and the name of the workflow.

```
$ hero query octocat/hello-world check.yaml
```

In this example, the repository is `hello-world` in the `octocat` account, and
the workflow is identified by its filename, `check.yaml`.

After a Run's Jobs are received, transformed into telemetry, and sent, a
record is made of this having been done on the local filesystem. This allows
**action-hero** to be re-run and only new Runs will be sent.

By default it will consider the most recent 10 Runs returned by the GitHub
API. To process more (or less) Runs pass a number via the `--count` option.

## Sending Telemetry

Traces and spans will be sent by the OpenTelemetry SDK, which defaults to
writing gRPC to a collector running locally at 127.0.0.1 port 4317 which then
can be configured to forward on to Honeycomb.

The best way to set this up is to download the _otelcol_ static binary from
the
[releases](https://github.com/open-telemetry/opentelemetry-collector/releases)
page of **open-telemetry/opentelemetry-collector** and once unpacked run with:

```
$ otelcol --config otel-collector-config.yaml
```

An example config file can be found in the _doc/_ directory; just enter an
appropriate Ingest Key for the Honeycomb environment you wish to send to.
Traces will appear in the `github-actions` service dataset.

## Use via webook

Instead of running **action-hero** on demand, you can instead configure it to
run as a webhook receiver, listening for HTTP connections coming from GitHub.


```
$ hero listen
```

The program will listen on port `34484` unless instructed otherwise with the
`--port` option. Since GitHub will expect to be talking HTTPS you should run
this program behind a reverse proxy such as Nginx with an appropriate
certificate installed.

## Development

It's difficult to develop a program like this because once you've processed a
given Run into a Trace at a given TraceId you can't send it again. Even if
TraceIds weren't deterministic you'd end up with multiple traces in your
dataset all at exactly the same point in time corresponding to whenever the
GitHub Action ran.

So, to facilitate development, there is an override which forward-dates the
beginning time of a Run to 10 minutes ago, and adds some randomness into the
TraceId so there won't be a collision. This allows you to simply reload the
query in Honeycomb and immediately find the trace that was just submitted so
you can iterate on the program. Invoke the override as follows:

```
$ RUST_LOG=hero=debug,*=warn HERO_DEVELOPER=true cargo run -- query octocat/hello-world check.yaml
```
