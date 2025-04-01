# Action Hero

_When you're doing Actions, the only thing that will save you is a Hero!_


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

## Usage

You pass the name of the repository (qualified with the owner or organization
it belongs to) and the name of the workflow.

```
hero octocat/hello-world check.yaml
```

In this example, the repository is `hello-world` in the `octocat` account, and
the workflow is identified by its filename, `check.yaml`.

After a Run's Jobs are recieved, transformed into telemetry, and sent, a
record is made of this having been done on the local filesystem. This allows
**action-hero** to be re-run and only new Runs will be sent.

By default it will consider the most recent 10 Runs returned by the GitHub API. To process more (or less) Runs pass a number via the `--count` option.

## Development

If you're trying to develop a program like this it's difficult to convert this
into accurate telemetry because once you've processed a given Run into a Trace
at a given TraceId you can't send it again. Even if TraceIds weren't deterministic you'd end up with multiple traces in your dataset all at exactly the same point some arbitrary amount of time in the past.

So, to facilitate development, there is an override which forward-dates the
beginning of the Run to 10 minutes ago, and generates additional randomness into the TraceId so there won't be a collision. This allows you to simply reload the query in Honeycomb and immediately find the trace that was just submitted so you can iterate on the program. Invoke the override as follows:

```
RUST_LOG=hero=debug,*=warn HERO_DEVELOPER=true cargo run -- octocat/hello-world check.yaml
```
