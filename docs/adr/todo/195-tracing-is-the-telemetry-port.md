# 195. `tracing` is the telemetry port; OpenTelemetry is a composition choice

## Purpose

Let a person see what happened inside a turn: which execution ran, which tools
it called, how long each took, where a permission waited, and what the provider
refused, across the panel, the gateway, the SDK and the agent process. Do it in
a way an embedder of `nessa-sdk` gets without our gateway, and without
telemetry ever becoming something the runtime depends on. The research is in
[design/agent_execution/telemetry.md](../../design/agent_execution/telemetry.md).

- **Date:** 2026-09-25
- **Status:** proposed
- **Argued in:** [#195](https://github.com/nessalabs/nessa-agent/issues/195)

## Context

The SDK already produces the semantic facts: a lossy `ExecutionUpdate`
broadcast for the UI and a mandatory, ordered `ExecutionAuditRecord` sink.
Neither carries a trace or span identity, and nothing in the workspace opens a
span. The gateway logs to one text file and records nothing on its product
socket path; the desktop host prints to a stderr a packaged app discards.
Correlation identities already exist and cross the wire: `conversationId`,
`executionId`, `requestId`.

Three rules bind the shape. There is no global service locator and ports return
Nessa-owned types, which rules out the OpenTelemetry crate's global provider and
its types on the SDK surface. Diagnostic logs are not an audit store, so
telemetry may be lost and audit may not, and neither may be built on the other.
The standards already name `tracing` as the SDK's diagnostics mechanism and
make the subscriber the executable composition's job.

## Decision

The SDK opens and closes `tracing` spans at the lifecycle sites it already owns
and emits one structured event per audit record, through one private
vocabulary module that names every span and field. Span handles live in the
structs that own the lifecycle they describe, so a span ends where the
lifecycle ends and there is no separate span map. Fields carry identities,
typed reasons, counts and sizes, never tool input, options, titles or message
text. The SDK never installs a subscriber and gains no OpenTelemetry
dependency. Each binary's composition installs one layered subscriber: text on
stderr, JSON trace files written off the request path by a bounded lossy
writer and rolled hourly with a fixed number kept, and an OpenTelemetry layer
through `tracing-opentelemetry` only when `OTEL_EXPORTER_OTLP_ENDPOINT` is
set, read through the process's environment seam. The first release gives one
trace per turn inside the gateway process; other processes are correlated by
identity fields until W3C context propagation lands as its own slice. Nothing
reads telemetry to decide anything, and no product feature renders from it.

## Alternatives considered

- **A Nessa `ExecutionObserver` port with a redacted `ExecutionSignal` enum
  and a `nessa-sdk-otel` adapter crate.** Type-enforced redaction and a typed
  surface for embedders, at the cost of a new public port, a new enum with
  three total constructors, a new crate with its own clock and span map, and a
  second API for embedders to learn beside `tracing`. Every piece re-implements
  something `tracing` and its bridge already do. Rejected as a single-use
  abstraction; the point at which it earns its place is named in the design
  document.
- **OpenTelemetry API directly in the SDK.** Puts a heavy dependency and a
  global provider at the core of a library, against the no-global rule, and
  forces every embedder to compile it. Rejected.
- **Derive telemetry from `subscribe()` plus `InvocationHook` outside the
  SDK.** Non-invasive, but the broadcast is lossy by contract and hooks do not
  see attachment, close, tool or permission events, so the trace would have
  holes the runtime could have filled. Rejected.
- **W3C context propagation across all three processes in the first
  release.** The shell emits no spans yet, and whether any harness forwards
  MCP `_meta` is unknown, so the propagation would be built against two ends
  that cannot yet use it. Deferred, and the promise is narrowed to match:
  correlation by `executionId` and `command_id` across processes, one real
  trace inside the gateway. The `traceparent` frame field and the MCP parent
  link are their own slices.

## Consequences

Easier: an embedder gets every span with one `tracing_subscriber` line and OTLP
with three; a new backend, host or tenant routing is composition work with no
SDK change; a new lifecycle fact is a constructor in one file plus the site that
owns the fact; the whole thing deletes without a public type disappearing.

Harder, accepted as cost: redaction is enforced by a marker-string test rather
than by a type; the subscriber is process-global, so two SDK instances in one
process share it and are told apart by `session_id`; metrics are derived from
spans downstream rather than emitted; the span vocabulary becomes a contract
embedders build on, so it changes additively. Model requests and token usage
stay invisible until a binding reports them, and the design says so rather
than faking a span.

Watch for: a product feature reading the trace files, a second consumer wanting
lifecycle facts as typed values, or a host needing per-instance subscriber
isolation. The first is a boundary violation; the other two are the signal to
grow the typed enum this record declined.
