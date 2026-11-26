# 1491. Telemetry is one dispatcher of generic events; turn origins are audit

## Purpose

Say where each chat turn came from (person and device, agent, event, app,
auto-pick, which client, typed or spoken) in the durable audit, and tell the
life of each turn and decision as OpenTelemetry spans when, and only when,
the person's configuration turns that on. Supersedes
[195](195-tracing-is-the-telemetry-port.md), which made `tracing` the port and
OpenTelemetry a subscriber layer.

- **Date:** 2026-11-26
- **Status:** proposed
- **Argued in:** [#1491](https://github.com/nessalabs/nessa-agent/issues/1491)

## Context

The SDK already records who initiated a turn (`Initiator`, `ActionContext`,
`MessageSender`) and the gateway can read a turn's origin (`turn_origins.rs`),
but no durable record held the origin as decided at send time, auto-pick was
not told apart from a person, and the handshake's client id (`nessa-browser`,
`nessa-cli`, `nessa-mcp`) and whether a message was spoken reached only a log
line. The only spans exported were decisions, through a `tracing-opentelemetry`
layer turned on by the mere presence of `OTEL_EXPORTER_OTLP_ENDPOINT`.

Two forces bind. Audit and telemetry are different promises: an audit record
must not be lost and may refuse the action it records; telemetry may be lost
and must never slow or refuse anything. And the owner wants more sinks later
(PostHog was named) without each one being a new layer wired into every
caller, and wants the telemetry crate to know nothing about Nessa.

## Decision

A new crate, `nessa-telemetry`, owns a generic `Event` (a name, a time,
attributes, and a shape: span start, span end, whole span, point, or
measurement, with spans named by caller keys) and one `Telemetry` dispatcher
that hands each event, off the caller's thread through a bounded queue that
drops and counts when full, to every sink that is on and wants its shape. The
OpenTelemetry sink (feature `otel`) derives trace and span ids from the
caller's keys, so a turn's span ends from a different call site, or after a
restart, in the trace it started in. Nessa's vocabulary lives in
`nessa-conversation` (`turn_telemetry.rs`, `decision_traces.rs`). Turn spans
are emitted from what is written, not from what is attempted: the submission
opens the span, and `ObservedExecutionAudit` wraps each agent's durable
execution audit and emits only after a record is written. Where a turn came
from is an audit record of its own (`turn-origins`), written before admission
and refusing the send when it cannot be written. `config.json`'s `telemetry`
section turns sinks on; absent, nothing runs and nothing leaves the process.

## Alternatives considered

- **Keep `tracing` as the port (ADR 195).** Every new sink becomes a subscriber
  layer with its own filter, an exporter is turned on by an environment
  variable rather than the person's configuration, and span identity lives in
  `tracing`'s in-process registry, so a span cannot be ended from another call
  site or after a restart. Rejected for the dispatcher, which is a few hundred
  lines and makes a sink one trait impl.
- **Put the origin on the telemetry span only.** Telemetry is lossy and off by
  default, so "who sent this" would be unanswerable exactly when it matters.
  Rejected: the origin is audit, and the span repeats it.
- **Emit turn spans from the SDK's live `ExecutionUpdate` broadcast.** It is
  lossy, per binding, and says what is happening rather than what was
  recorded, so a span could say a turn settled that the audit does not hold.
  Rejected for the audit wrapper.
- **A separate repository for the telemetry crate.** Asked about; kept in this
  workspace so it versions with its callers. It has no Nessa dependency, so
  moving it later is a copy.

## Consequences

Easier: adding a sink (implement `Sink`, add it in `composition/telemetry.rs`);
answering "who sent this turn" from the audit for any send since this landed;
following one turn across admission, dispatch, end and resumption in a trace
viewer.

Harder: `OTEL_EXPORTER_OTLP_ENDPOINT` alone no longer exports anything, which
changes behaviour for anyone who relied on it for decision spans; they add
`"telemetry": {"otel": {"enabled": true}}`. Every send writes one more small
file before admission. Tool and model spans from the SDK's GenAI trace store
are not forwarded yet; that is a follow-up, since those stores are per binding
and time from a monotonic origin.

Watch for: the dropped-event count logged at exit growing (the queue is too
small or a sink is slow), and the turn-origin directory's size on very busy
gateways (one file per attempt, like file-link evidence).

## States

| Situation | Outcome |
| --- | --- |
| No `telemetry` section, or `otel.enabled` false | `Telemetry::off()`: no thread, no queue, every emit returns at once. Origin audit records are still written. |
| `otel.enabled` true, collector reachable | Turn spans (`nessa.turn`) and decision spans (`decision`) are exported over OTLP/HTTP with service `nessa-gateway`. |
| `otel.enabled` true, endpoint not an http(s) URL | `config.json` is refused at startup (`RuntimeConfig`), like any other malformed section. |
| `otel.enabled` true, exporter cannot start or collector down | Logged once; the gateway runs the same. Spans are lost, never retried into the audit. |
| A fresh send | Its origin is read, written as a `conversation_turn_origin` record, then the turn's span starts. A record that cannot be written refuses the send. |
| An origin that cannot be read | Recorded as `unread`; the send is not refused for it. |
| The same attempt retried | The record reconciles with the one it left. A retry claiming another origin is refused. |
| A send refused before admission | Its span ends with outcome `refused`. |
| Admitted and run | Point `nessa.turn.admitted`, then the span ends when the provider's `Finished` record is written, with its outcome (`completed`, `output_limit`, `request_limit`, `refused`, `cancelled`) or `failed` and the cause's code. |
| Admitted, waited in the queue, then run | As above, with point `nessa.turn.dispatched` when it leaves the queue. |
| Settled, cancelled or injected before it ran | The span ends with the queue's stage and cause. |
| Any of the above | Told only after its audit record was written. A turn's span is named by its execution id, the one id every record of it carries (`Finished` names the provider's session, not the conversation). |
| A turn resumed after a restart or failure | The successor's span starts linked to the one it continues. |
| A decision asked, paused, settled, or its conversation erased | Span start, a point, then an end with outcome and timing, or an end with no outcome. One settled after a restart is told as one whole span marked `decision.reopened`. |
| The queue is full | The event is dropped and counted; the count is logged at exit. |
| Process exit | What is queued is flushed, waiting at most five seconds. |
