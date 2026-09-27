# Telemetry for Nessa: SDK, gateway, desktop

Status: proposed. Research behind
[ADR 195](../../adr/todo/195-tracing-is-the-telemetry-port.md), argued in
[#195](https://github.com/nessalabs/nessa-agent/issues/195). Nothing here is
implemented; each slice in section 5 is a sub-issue of #195. Paths are
relative to the repository root. Line numbers were read at `19d42ff1`.

## 1. What we want

Three consumers, one mechanism:

1. **Us, debugging our own app.** When a turn hangs, a permission never shows,
   or the gateway restarts, we need one trace from the gateway request through
   the SDK's execution, and the agent process's tool runs joined to it by IDs
   we already mint. One trace across all processes needs W3C context
   propagation, which is a later slice (3.4).
2. **Us, debugging agents.** Per execution: which tools ran, how long, which
   reviews were requested and answered, where the provider stalled, what the
   provider refused. Token usage when the provider reports it.
3. **SDK users without our gateway.** A Rust host that embeds `nessa-sdk` gets
   the same signals through the `tracing` subscriber it already installs, and
   OpenTelemetry through the standard `tracing-opentelemetry` bridge. They
   pick the exporter. We never pick it for them.

The architectural rule holds and matches the repo's own
standards: **the runtime emits semantic events; a telemetry observer translates
selected events into spans, metrics and logs; the runtime never knows where
telemetry goes.** Durable audit and telemetry are separate channels. Losing a
span is fine. Losing an audit record is a bug.

## 2. What exists today

### 2.1 The SDK already has the semantic event stream

There is no `TurnAccepted` / `ExecutionStarted` family. ADR 0008's
`ConversationEvent` and `TurnState` are proposals, not code. What exists is
richer than the usual "turn started, tool started" sketch in places and thinner in others:

| Live observation (lossy broadcast, `subscribe()`) | Mandatory audit (`ExecutionAudit::record`, ordered, durable) |
|---|---|
| `ExecutionEvent { execution_id, update: ExecutionUpdate }` | `ExecutionAuditRecord` |
| `Message(MessageChunk)` text or thought | `Attachment{session_id, generation, before, after, cause, actor}` |
| `Tool(ToolCallUpdate{id, title?, kind?, status?, ...})` sparse | `QueueAdmitted`, `QueueSettled`, `QueueReordered` |
| `PermissionRequested{id, tool_id, observation, input, options}` | `SteeringAcknowledged` |
| `PermissionCancelled(PermissionCancellation)` | `Finished(ExecutionFinish)` |
| `ReviewDeclined(ReviewDeclineObservation)` | `SessionClosed`, `Cancelled`, `Answered`, `ReviewDeclined` |
| `Finished(ExecutionOutcome)` terminal | |

Sources: `crates/nessa-sdk/src/application/agent_execution/executions/events.rs:170-198`,
`executions/audit.rs:460-499`.

Facts that matter for the design:

- The broadcast channel has capacity 256 and returns `AgentError::Backpressure`
  to a lagging subscriber (`agents/agent.rs:137`, `:1154-1160`). It is lossy by
  contract. The gateway's projection loop already handles `lagged()`
  (`crates/nessa-server/src/conversation/application/service.rs:818-829`).
- Every publish site saves first, then broadcasts (`agent.rs:603`, `:882`).
- `InvocationHook` (`hooks/invocation.rs:40-67`) sees only before/after of one
  invocation. It does not see attachment, close, tool, or permission events, and
  its docs say it "is not an audit sink".
- Nothing carries a model request or token usage. The ACP worker maps only
  `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`
  and permission requests; every other `sessionUpdate` kind is dropped at
  `infrastructure/acp/executions/worker.rs:1807`. `estimated_input_tokens` is a
  caller estimate (`executions/request.rs`). So `ModelRequestStarted/Finished`
  cannot be honest today. The harness owns the model loop
  (ADR 0012:37-42); we only see what the binding reports.
- There are no spans anywhere in the workspace. `tracing` is used for about 23
  event-style calls in the SDK and about 60 in the server. No `#[instrument]`,
  no `Span::current`.
- Correlation IDs already exist and are minted at known places:

| ID | Minted where | Crosses the wire? |
|---|---|---|
| `SessionId` (conversation) | `sessions/manager.rs:157` or caller | yes, `conversationId` |
| `ExecutionId` | caller; the client mints a UUID per send (`src/conversation/adapters/store/slice.ts:150-152`) | yes, `executionId` |
| `ActionContext{principal_id, surface_id, request_id}` | gateway from credential + frame id (`product/conversation.rs:62-67`) | yes, `requestId` |
| Attachment `generation: u64` | SDK lifecycle counter | no |
| `PermissionId`, `ToolCallId`, `MessageId` | provider | no |
| `EndpointIdentity` (instance UUID + pid) | gateway at bind, exposed on `/health` | yes, headers |
| MCP `commandId`, `connectionId`, `requestId` | `nessa-mcp` audit | no |

  There is no trace ID and no span ID concept anywhere.

### 2.2 The gateway logs to one file and streams nothing

- One `tracing_subscriber::fmt` on stderr with `RUST_LOG` or the default
  `nessa_server=info,nessa_sdk=warn,nessa_gateway_endpoint=error`
  (`crates/nessa-server/src/core/logging.rs:15-30`). Plain text. Under launchd
  stderr is `<data>/logs/gateway.log`, truncated once at start
  (`core/log_file.rs`). The plist never sets `RUST_LOG`, so production is always
  at the default filter.
- `logging::init()` runs before `Environment::load` (`core/bootstrap.rs:9`), and
  `RUST_LOG` is read directly, bypassing the `env` seam (`logging.rs:29`).
- The product socket, auth, dispatch and close paths log nothing at all. No
  request, latency, or close reason is recorded.
- Conversation updates are **pulled**: the client polls `conversation.read`
  every 250 ms (`src/conversation/adapters/gateway/polling.ts`), and the server
  folds SDK events into a bounded `Projection`. The only push is
  `session.challenge`. There is no per-event wire record to hang trace context
  on.
- `RequestFrame` is `deny_unknown_fields` (`protocol/frames.rs:9-17`), so a
  `traceparent` on the envelope is a schema change under the one-contract rule.
  Per-method params already carry `requestId` and `executionId`, which is where
  correlation lives today.
- `GET /health` deliberately "exposes no product state" (`server/entrypoint/http.rs:13-14`).
  Not a place for a metrics endpoint.

### 2.3 The desktop host and shell have no logging framework

- Host: about 67 `eprintln!("[nessa] ...")` calls, no `tracing`, no `log`
  crate. In a packaged app that stderr is lost. The only structured evidence
  is the reconciliation audit (`src-tauri/src/gateway/infrastructure/reconciliation_audit.rs`)
  and the credential-save audit, both JSON files under the config root.
- Shell: `console.warn/error` with a `[nessa]` prefix; a dev-only bridge
  forwards warn/error to the terminal (`src-tauri/src/diagnostics.rs`,
  `src/diagnostics/dev-console.ts`). No timing code anywhere.
- No "open logs", no support bundle, no diagnostics export in the tray or
  settings.

### 2.4 nessa-mcp

- Stdout is JSON-RPC only. Subscriber is `fmt` on stderr, fixed at INFO, no env
  filter (`crates/nessa-mcp/src/main.rs:18-20`).
- `ShellService::run` already brackets each command with
  `Evidence::Admitted | Started | Finished(RunResult)` through an `Audit` port
  (`shell/application/service.rs:42-84`, `ports.rs:8-14`). That is a
  start/finish observer port in all but name.
- `tools/call` accepts `_meta` and ignores it (`mcp.rs:32-33`). MCP reserves
  `_meta` for exactly this kind of context propagation.

### 2.5 The standards that constrain the design

Each of these is already a rule; telemetry has to fit them, not the reverse.

- **No global service locator, no mutable process-wide handle**
  (`CODING_STANDARDS.md:470-476`). The OTel SDK's `global::tracer_provider()`
  pattern is out. The provider is built in composition and injected.
- **Ports return Nessa-owned types** (`:478-482`). The SDK's public port must not
  expose `opentelemetry` or `tracing` types.
- **Clock reads go through an injected port** (gate 9, `:34-43`). Span
  timestamps in the SDK need a clock port; the SDK has none today
  (`Instant::now()` in `worker.rs`, `binding.rs:647`, `scheduling.rs:501`).
- **Survivable edge failures stay survivable** (gate 7). An exporter that is
  down, slow, or misconfigured must never fail a turn.
- **Diagnostic logs are not a durable audit store** (`:565`). Audit must not be
  built on the telemetry pipeline, and telemetry must never be needed for
  recovery, permission evidence, or correctness.
- **"A diagnostic is not a decision"** (`:178`). Nothing reads telemetry to
  decide anything.
- **No raw tool arguments, credentials, or thinking in general logs**
  (`:572-573`, ADR 0014:108-109). Spans carry tool identity, kind, status,
  sizes, and typed reasons. Never `ToolReviewInput`, never message text.
- **Typed failures** (gate 1). Span status and `error.type` come from enum
  variants, not `Display` strings.
- **One owner of a decision** (gate 13). Telemetry derives from published
  events; it never runs its own state machine for turn state.
- **No second in-memory broadcast, no event bus**
  (`docs/design/agent_execution/runtime-classes-and-sequences.md:221-224`).
- **Background adapters expose explicit startup/shutdown owned by composition**
  (`docs/design/dependency-injection.md:46-49`). The batch exporter's flush is
  composition's job, in the shutdown sequence, not a `Drop`.
- **A per-instance SDK**: two SDK instances keep state and adapters separate
  (ADR 0008:1006-1010). Telemetry state is per `Agent`, not per process.
- **Use `tracing` for SDK diagnostics; executable composition initialises the
  subscriber; the desktop host keeps stderr until it is wired**
  (`CODING_STANDARDS.md:702-705`).
- **Do not scaffold empty layers; add abstractions for an actual consumer**
  (`dependency-injection.md:111-113`).

## 3. Proposal: `tracing` is the port

The first draft of this document added an `ExecutionObserver` port, an
`ExecutionSignal` enum, a `nessa-sdk-otel` crate, a `RunObserver` port in
`nessa-mcp`, a `HostTelemetry` port in the desktop host and a `Telemetry` port
in the shell. Every one of those re-implements something `tracing` already
does: the SDK depends on it, the standards name it as the diagnostics
mechanism (`CODING_STANDARDS.md:702-705`), and the ecosystem bridge to
OpenTelemetry (`tracing-opentelemetry`) is the standard way Rust libraries get
exported without knowing about exporters. Appendix A lists what was cut and
what each cut costs.

### 3.1 Shape

```
  nessa-sdk (library)                       host composition (binary)
  ┌──────────────────────────────────┐      ┌───────────────────────────────────┐
  │ Agent / scheduler / ACP worker   │      │ tracing subscriber, built once:   │
  │   opens and closes tracing spans │emit  │   fmt text ──▶ stderr / *.log     │
  │   at the lifecycle sites it      │─────▶│   fmt json ──▶ logs/trace/ rolling │
  │   already owns; emits events     │      │   otel layer ─▶ OTLP (opt-in)     │
  │   with typed fields, never text  │      │   any Layer the embedder writes   │
  └──────────────────────────────────┘      └───────────────────────────────────┘
        ▲ same in nessa-mcp and src-tauri          ▲ gateway, nessa-mcp, src-tauri
```

The SDK **emits** and never installs a subscriber. Each binary's composition
installs one. Nothing new crosses the SDK's public API: no port, no enum, no
crate. An embedder without our gateway installs whatever subscriber they like;
that is the same contract they already accept for the SDK's 23 diagnostic
events.

### 3.2 Spans and events in the SDK

Spans are held by the object that already owns the lifecycle they describe,
so they end where the lifecycle ends and there is no separate span map (gate
13: one owner of "is this execution live").

| Span or event | Opened where | Held by | Closed where |
|---|---|---|---|
| `nessa.session` | `Agent::prepare` | `Agent::Inner` | `close()` completes or last handle drops |
| `nessa.attachment{generation}` | `start_attachment` | `SessionLifecycle` attachment state | attached / failed / closed transition |
| `nessa.execution{execution_id, kind, provider}` | admission (`enqueue`, `enqueue_steering`, `invoke`), parent = caller's current span | `Scheduler::pending[id]` then `ActiveInvocation` (`scheduling.rs:197-240`) | `ActiveInvocation::drop`, which already calls `finish_execution` |
| `nessa.tool{tool_id, kind}` | first `ToolCallUpdate` for an id | the domain `ToolCall` entity's application wrapper | status `Completed` or `Failed`, or execution end |
| `nessa.permission{permission_id, tool_id}` | `PermissionRequested` | permission request entry | `Answered` or `Cancelled{reason}` |
| event `audit.record{kind, ...ids}` | `observation::record_audited`, the one function every sink call traverses | | |
| event `audit.delivery_failed{kind, error}` | same helper, on `Err` | | |
| event `provider.refused{code, phase}` | worker | | |
| event `output.chunk{kind, bytes}` at `trace` level | worker | | |

**The vocabulary lives in one file.** Span and event constructors are private
functions in one module, `application/agent_execution/observation.rs`:
`session_span(&SessionId)`, `execution_span(&Pending)`, `tool_span(..)`,
`permission_span(..)`, `audit_fields(&ExecutionAuditRecord)`,
`error_kind(&AgentError)`, and the name constants they use. Emit sites call
a constructor; they never spell a span name or a field list themselves. This
is the "definition from execution" separation: the description of what
telemetry exists is one inspectable file, and running it is the emit sites.
It is not a port and not public; it is the one implementation of a
cross-cutting concern, applied at the boundary. The redaction test targets
this module, `tracing.md` is written from it, and every vocabulary change
(a new field, a rename when `Turn` lands, a `gen_ai.usage.*` field when a
binding reports tokens) is a one-file change plus the emit site that gains
the new fact.

Rules that hold at every emit site:

- **Fields are identities, typed reasons, counts and sizes.** No tool input,
  no options, no titles, no message text. `Display` of an `AgentError` is
  not a field; its variant name is, through a total `match`
  (`error_kind(&AgentError) -> &'static str`) so a new variant is a compile
  error (gate 11). Enforcer for the redaction rule: a collecting `Layer` in
  tests runs a full execution with marker strings in the tool input, options
  and output, and asserts no recorded field contains a marker.
- **Audit records get one emit site, and every sink call traverses it.**
  There are three owners that call the sink today: the scheduler (six calls
  in `agents/scheduling.rs`), `SessionLifecycle::record_attachment_audit`
  (`agents/lifecycle.rs:574`) and the ACP worker's `record_audit`
  (`infrastructure/acp/executions/worker.rs:2174`). Between them they emit
  every record kind, so a helper on `Agent` alone would miss attachment,
  finish, close, cancellation, answer and review-decline records. Instead
  `observation.rs` owns one free function,
  `record_audited(&dyn ExecutionAudit, ExecutionAuditRecord) -> AgentFuture<()>`,
  that emits `audit.record` through `audit_fields`, a total `match` over the
  variants, calls the sink, and emits `audit.delivery_failed` on `Err`. All
  three owners call it; the panic and timeout wrappers they already apply
  wrap the returned future exactly as they wrap the sink's today. The
  enforcer is structural: a source-scanning test in the crate asserts that
  `.record(` on the `ExecutionAudit` port appears in exactly one function,
  `record_audited`. Result: every audited transition is observable, a new
  record variant cannot compile unobserved, and a new caller cannot bypass
  the emit.
- **Cross-task continuity is explicit.** Attachment and steering run in
  spawned Tokio tasks (`agent.rs`), so the SDK `.instrument(span.clone())`s
  those futures with the owning span. Nothing relies on `Span::current()`
  surviving a spawn.
- **The subscriber never changes an outcome.** `tracing` calls are
  synchronous and infallible from the library's side. A subscriber that
  panics is the host's defect and is outside the SDK's contract; the test
  `slow_subscriber_does_not_change_outcome` still runs with a deliberately
  slow `Layer` and asserts identical outcome, audit records and snapshot.
- **Clock.** The SDK reads no clock for telemetry; span timestamps are the
  subscriber's, which is composition (gate 9 is satisfied because the read
  happens outside the library, behind `tracing::Dispatch`).

What this gives an SDK user with three lines in their `main`:

```rust
tracing_subscriber::fmt().json().with_span_events(FmtSpan::CLOSE).init();
// or: .with(tracing_opentelemetry::layer().with_tracer(otlp_tracer))
```

Every execution, tool and permission becomes a span with duration; every
audit record becomes an event. No Nessa-specific adapter, no second API to
learn.

### 3.3 Mapping to OpenTelemetry

Unchanged from the first draft in substance; `tracing-opentelemetry` performs
it. Span names and fields above already follow `gen_ai.*` for provider and
model attributes, `error.type` for typed failures, and the `nessa.*`
namespace otherwise. Metrics are **derived, not emitted**: execution and
tool duration histograms, permission wait, audit delivery failures and
provider refusals are all countable from spans and events by every backend
we care about (Tempo, Honeycomb, Datadog, Langfuse, Grafana span-metrics) and
by a `jq` over the rolling trace files. If a first-class meter is ever needed,
`tracing-opentelemetry`'s `MetricsLayer` reads `counter.*` and `histogram.*`
fields from the same events. Nothing changes in the SDK for that.

Still honest about the gaps: no `model.generate` spans and no token usage
until a binding reports them (`worker.rs:1807` drops those updates;
ADR 0012:37-42). When it does, it is one more field on `nessa.execution`,
`gen_ai.usage.*` with a `source=measured|estimated` field per `sdk-shape.md:173`.

The trace for one turn through the gateway:

```
rpc conversation.send                (gateway span: request_id, principal_id)
└── nessa.execution                  8.4s  session_id, execution_id, kind=queued
    ├── nessa.tool  kind=read         0.08s
    ├── nessa.tool  kind=execute      2.1s
    │   └── nessa.permission          1.6s  (human wait)
    └── nessa.tool  kind=edit         0.05s

mcp.shell.run                        0.4s  (nessa-mcp: its own trace, fields
                                            command_id and the traceparent it
                                            was handed; a backend joins it by
                                            field, not by parent)
```

That is two traces joined by a field until W3C propagation lands. The
gateway-to-SDK half is a real parent and child because it is one process.
The MCP half becomes a child only when 3.4 step 3 is done in full.

### 3.4 Context across processes

The first release promises one trace per turn inside the gateway process
and separately correlated traces elsewhere. Sharing an ID as a field lets a
backend search across traces; it does not make one span the parent of
another. Making the whole path one trace is W3C context propagation, and
each hop below says which of the two it delivers.

1. **Shell → gateway: correlation only.** The shell emits no spans in the
   first release, so the gateway's RPC span is the trace root. It records
   `request_id` and `execution_id`. When the shell does emit spans, a W3C
   `traceparent` on the frame makes the gateway span a remote child; that is
   an optional field in `frames.json`, not a version bump
   (`CODING_STANDARDS.md:445-455`), and it is its own sub-issue.
2. **Gateway → SDK: real parent and child.** The gateway calls `enqueue`
   inside its RPC span, so the execution span's parent is the request span.
   Same process, no map, no glue.
3. **SDK → agent → nessa-mcp: correlation first, propagation when the
   harness allows it.** In full, this hop is: the SDK writes a `traceparent`
   for the execution span into the ACP `session/prompt` `_meta`, the harness
   forwards `_meta` on its `tools/call`, and `nessa-mcp` installs it as the
   remote parent of `mcp.shell.run` through `tracing-opentelemetry`'s
   `set_parent`. Whether any harness forwards `_meta` is unknown and is
   recorded as a tri-state capability like the hook capabilities. The first
   release does the two ends we control: the SDK writes the header, and
   `nessa-mcp` records whatever `traceparent` it receives as a field. The
   parent link is installed in the slice that proves a harness forwards it.
4. **Gateway ↔ desktop host.** `EndpointIdentity` becomes the gateway's
   `service.instance.id`; the host records it and its reconciliation
   `correlation_id` on the same spans it already audits.

### 3.5 What each process does

**nessa-sdk** (one slice)

- The private vocabulary module `observation.rs`; spans and events at the
  sites in 3.2; the private `audit` helper; the `.instrument` on spawned
  tasks. The emit lines land inside `agents/`, `sessions/`, `executions/` and
  the ACP worker where those lifecycles already live. `mod.rs` maps gain one
  sentence each.
- Three structural absences, each pinned by a source-scanning test in the
  crate (the repo's pattern for invariants that erode silently): no
  `tracing::span!`/`info_span!` in `domain/`; no `tracing_subscriber` outside
  `examples/` and `#[cfg(test)]`; no `opentelemetry` anywhere in the crate.
- Tests: the collecting-layer redaction test, the slow-layer outcome test,
  and a span-lifecycle test over the ordering matrix
  (`CODING_STANDARDS.md:182-190`) asserting one `nessa.execution` close per
  open across normal completion, explicit close, provider withdrawal,
  deadline and dropped handles.
- Docs: `crates/nessa-sdk/docs/agent_execution/tracing.md` naming every span
  and field and stating what a span does not prove; a row in that README.
  `examples/claude_acp.rs` shows the JSON subscriber. A second example,
  `examples/otel.rs`, shows `tracing-opentelemetry` with an OTLP exporter,
  using dev-dependencies only, so the SDK crate itself gains no OTel
  dependency.

**nessa-server (gateway)**

- Env seam: no Nessa-invented switch. The standard
  `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_EXPORTER_OTLP_HEADERS` names and
  `RUST_LOG` are read through `EnvSource` into `Environment`; OTLP export is
  on exactly when the endpoint is set, which is the OTel convention every
  operator already knows. The exporter builder gets values and has its own
  env lookup disabled. Interpretation is
  `TelemetryConfig::from_environment(&Environment)`, a pure function with
  tests. `NESSA_RUNTIME_FINGERPRINT` and `NESSA_AGENT_PATH` move behind the
  same seam because they are the same defect.
- Bootstrap: load `Environment`, then build one layered subscriber: `fmt` text
  on stderr (unchanged, so `gateway.log` stays the artefact a person sends
  us), `fmt` JSON with `FmtSpan::CLOSE` to `<data>/logs/trace/` (below), and
  `tracing-opentelemetry` over OTLP only when an endpoint is configured. With
  an endpoint, composition owns the provider and calls `shutdown()` after
  `ConversationService::shutdown` in the graceful path
  (`dependency-injection.md:46-49`).
- The trace file is written off the request path. A plain `fmt` layer
  formats and writes inside `on_event`, on whichever task closed the span,
  so a stalled filesystem would stall a turn. The JSON layer therefore
  writes through `tracing_appender::non_blocking` in lossy mode: a bounded
  channel (128k lines) to one writer thread; when the channel is full the
  line is dropped and counted, never awaited. Composition holds the
  `WorkerGuard` and drops it in the graceful shutdown path after the OTLP
  `shutdown()`, which flushes the channel; at that point it reads the
  appender's dropped-line counter and, if non-zero, writes one text line to
  `gateway.log` saying how many trace records were lost. The text layer on
  stderr stays synchronous as today: its volume is a handful of lines an
  hour and it is the last thing that must still work when everything else
  is broken.
- The trace file rolls while the process runs. `gateway.log` is truncated
  only at start, which `core/log_file.rs:32-36` already says does not bound
  a single long run; that is acceptable for a log that writes a few lines an
  hour and not for one that writes a line per span close. `trace/` is a
  `tracing_appender::rolling` appender with hourly rotation and
  `max_log_files(24)`, so the on-disk set is bounded by count and age
  during the run, not only at restart. The per-file size is bounded by
  volume, not enforced: one span close is about 1 KB, so a gateway
  running 1,000 executions an hour with ten tools each writes about 11 MB
  an hour and holds about 260 MB at most. If that is too much for a
  machine, `RUST_LOG` lowers what reaches the layer; a byte-size rotation
  would be a later change to the same appender. `tracing-appender` is the
  Tokio project's own crate and adds no transitive dependencies the server
  does not already build.
- `#[instrument(skip_all, fields(rpc.method, request_id, principal_id))]` on
  `dispatch` (`product/socket.rs:367`) gives the per-RPC span. A `warn!` at
  the `lagged()` branch (`service.rs:826`) records projection loss.
- The plist sets the `OTEL_*` values, when a user configures them, from
  `service_environment` (`src-tauri/src/gateway/infrastructure/macos.rs:505-535`).

**nessa-mcp**

- One `info_span!("mcp.shell.run", command_id, traceparent)` in
  `ShellService::run` around the existing audit bracket
  (`shell/application/service.rs:64-84`). The `Audit` port is untouched.
- Add the `env-filter` feature to its subscriber so `RUST_LOG` works;
  keep the writer on stderr because stdout is JSON-RPC.
- Config by argv only, as today; the gateway passes `--log-level` from
  `composition/desktop.rs:152-165`.

**src-tauri (desktop host)**

- The migration `CODING_STANDARDS.md:702-705` already anticipates: build a
  `fmt` subscriber in `main.rs` before `Builder`, text to
  `<config_root>/logs/host.log` bounded on start, and replace the 67
  `eprintln!("[nessa] ...")` calls with `tracing::{info,warn,error}!`.
  Failures before the subscriber exists keep `eprintln!` and say so.
- Spans where the host already has correlation IDs: gateway reconciliation
  and readiness (`macos/control.rs:870-921`), update check, summon-to-shown.
  No port: the subscriber is composition's, exactly as in the gateway.
- Tray item "Reveal diagnostics" opens the config root's `logs/` folder.

**React shell and nessa-client** (last, smallest)

- No new port until there is a consumer beyond one timing. The gateway's RPC
  span already measures `conversation.send` end to end on the server side.
  When UI-side latency is wanted, a `now()` clock joins `createDependencies`
  and the shell logs one structured `console.info("[nessa] ...")` per
  send-to-visible, which the existing dev bridge forwards and a release host
  command appends to `host.log`.

### 3.6 Defaults

| Process | Default | Override |
|---|---|---|
| gateway | text log + rolling JSON trace files; OTLP off | `OTEL_EXPORTER_OTLP_ENDPOINT` turns OTLP on; `OTEL_EXPORTER_OTLP_HEADERS`, `RUST_LOG` via plist |
| desktop host | text `host.log` | none in the first slice |
| nessa-mcp | stderr, INFO | `--log-level` argv |
| SDK embedder | nothing installed | whatever subscriber they build |

Telemetry never opens a socket by default. The trace file is written by a
separate thread through a bounded, lossy channel, so filesystem stalls cost
dropped trace lines, not turn latency; the enforcer is named in section 6.

### 3.7 Out of scope, unchanged

No decision reads telemetry. No audit port changes. No native harness hooks.
No hosted telemetry service. No metrics API in the SDK.

### 3.8 Extensibility and growth

Judged by the cost of the next correct change, per the `system-architect`
skill. The question for
each scenario is how many decisions it touches.

| Next change | What changes | Files |
|---|---|---|
| A field is added to an existing span (a new typed reason, a size) | one constructor in `observation.rs` and the emit site that has the fact | 2 |
| A binding starts reporting model requests and token usage | `gen_ai.usage.*` and a `model_span` constructor in `observation.rs`; the binding's worker emits them. The core gains a general facility (a span for a provider-reported request), not knowledge of that binding | 2 |
| `Turn` lands (ADR 0008) | `turn_span` constructor; the turn owner opens and closes it around executions. `nessa.execution` keeps its name; the turn is additive | 2 |
| Hooks land (ADR 0014) | `hook_span{rule, verdict}` constructor; emitted at the Nessa-owned hook boundary, never from native harness hooks | 2 |
| A new provider binding | uses the existing constructors from its worker; no vocabulary change unless it reports a new kind of fact | 1 |
| A new backend (Langfuse, Datadog, a file on a support server) | composition only, in the binary that wants it: an exporter and, if the backend wants renamed attributes, an OTel processor there | 0 in the SDK |
| A new host (CLI, second Rust server, background runner) | installs a subscriber in its own `main`; inherits the whole vocabulary | 0 in the SDK |
| Hosted, multi-tenant gateway | the gateway's own RPC span carries `organization_id`; per-tenant routing or sampling is an exporter decision in composition. The SDK never learns tenancy (§8 of the skill: the core does not learn the product's vocabulary) | 0 in the SDK |
| `traceparent` on the wire | one optional field in `frames.json`, regenerated; the gateway sets the RPC span's parent from it | 2 |
| Trace volume becomes a cost | a `Sampler` on the OTel layer in composition; chunk events are already at `trace` level and filtered by default | 0 in the SDK |
| A first-class metric is wanted | `MetricsLayer` in composition reads `counter.*`/`histogram.*` fields; the field names join `observation.rs` | 1 + composition |
| Telemetry is deleted | delete `observation.rs` and the emit lines; delete the JSON and OTel layers from each `main`. No public type or port disappears, no embedder breaks | bounded |

What the structure refuses, and why that is the growth story rather than a
limitation:

- **Telemetry is not a product input.** The shell never renders a timeline
  from the trace files; a "what did the agent do" feature reads session
  snapshots and audit, which are durable and authoritative. The moment a
  product feature reads spans, span shape becomes a contract with a UI, and
  the vocabulary stops being cheap to change. This is the "diagnostic is not
  a decision" rule (`CODING_STANDARDS.md:178`) read as a growth constraint,
  and it is pinned by a structural absence test.
- **No modes in the core.** There is no telemetry on/off flag, no verbosity
  parameter, no "which backend" anywhere in the SDK. `tracing`'s level filter
  and the subscriber are the only knobs, and both belong to the host. Every
  flag we do not add is a state space we do not test.
- **The vocabulary is a contract, so it changes additively.** Once embedders
  build dashboards on `nessa.execution`, a rename breaks them. New facts get
  new fields or new spans; existing names stay until the one-contract rule
  says otherwise. `tracing.md` is the published surface and is written from
  `observation.rs`.
- **The span map that was cut stays cut.** Holding spans in `Pending` and
  `ActiveInvocation` means "which executions are live" has one owner. If a
  future change needs telemetry state the lifecycle does not have, that is
  the signal to add the fact to the lifecycle, not to grow a side table.

Where the design would be re-cut if the pressure arrives:

- If two independent consumers need the lifecycle facts as typed values
  rather than as `tracing` fields (for example a durable event stream per
  ADR 0009 *and* a hook context per ADR 0014 both wanting one record shape),
  that is the point at which a public `ExecutionSignal`-style enum earns its
  place. The `audit_fields` total match in `observation.rs` is where it
  would grow from. Not before: a single-use abstraction is refused.
- If a host needs true per-instance subscriber isolation, `tracing`'s
  `Dispatch` scoped around that instance's tasks is the mechanism; it is
  composition work and needs no SDK change.

## 4. Risks and open questions

1. **The subscriber is process-global.** That is the one global the
   standards already accept for `tracing`. Two SDK instances in one process
   share it and are told apart by `session_id`. A host that needs true
   isolation can scope a `Dispatch` with `tracing::dispatcher::with_default`
   around its instance; we do not build that.
2. **Redaction is enforced by a test, not a type.** The first draft's
   `ExecutionSignal` made leaks unrepresentable. Here the marker-string test
   is the enforcer and is listed in section 6. The trade is a public enum and
   three total constructors for one test.
3. **Span handles inside lifecycle structs.** `Pending` and
   `ActiveInvocation` gain a `tracing::Span` field. `Span` is `Clone`, cheap
   and `Send`, and a disabled subscriber makes it a no-op, so cost is nil
   when nothing listens. The domain layer stays free of it: spans live in
   application and infrastructure structs only.
4. **`traceparent` on the wire** stays deferred; attribute joining is enough
   for the first release.
5. **Harness `_meta` forwarding** is unknown; recorded as a capability.
6. **Trace file growth and loss.** Hourly rolling with 24 files kept bounds
   the set by count and age while the process runs; per-file size is bounded
   by volume with the estimate in 3.5, not enforced. The non-blocking writer
   drops lines when its channel is full and reports the count at shutdown;
   a dropped trace line is acceptable by design, and the count is how we
   learn the buffer is too small. Span close events at `info`, chunk events
   at `trace` and filtered out by default.

## 5. Delivery slices

| # | Slice | Touches |
|---|---|---|
| 0 | [#195](https://github.com/nessalabs/nessa-agent/issues/195) and [ADR 195](../../adr/todo/195-tracing-is-the-telemetry-port.md): `tracing` spans are the SDK's observation contract, no custom observer port, OTel is a host composition choice, `traceparent` deferred. This document and the index rows. `docs/ARCHITECTURE.md` gains its telemetry paragraph with slice 1, when it becomes true. | docs |
| 1 | SDK spans, events, `audit` helper, `.instrument`, the three tests, `tracing.md`, two examples. | nessa-sdk |
| 2 | Gateway env seam and bootstrap reorder, layered subscriber, rolling non-blocking trace file with dropped-line report, `#[instrument]` on `dispatch`, `lagged` warn, OTLP shutdown, plist env. | nessa-server, plist |
| 3 | nessa-mcp span, `_meta.traceparent` field, env-filter. | nessa-mcp, desktop composition |
| 4 | Host `tracing` wiring, `host.log`, `eprintln!` migration, spans, tray item. | src-tauri |
| later | `gen_ai.usage.*` when a binding reports tokens; `traceparent` in `frames.json`; shell timing; `MetricsLayer` if a meter is ever needed. | |

Four code slices instead of seven, zero new crates, zero new public SDK
types. CI: the new tests run in the existing `cargo test -p nessa-sdk` and
server suites; no new job (`CODING_STANDARDS.md:714-740`).

## 6. Claims and their enforcers

| Claim | Enforcer |
|---|---|
| Telemetry cannot change an execution's outcome, audit records or snapshot | `tracing` macros are synchronous and return `()`; test `slow_subscriber_does_not_change_outcome` compares against a no-op subscriber |
| No tool input, options, titles or message text reach any field | test `no_marker_reaches_subscriber` with a collecting `Layer` and marker strings in input, options and output |
| Every audited transition is observable | `observation::record_audited` is the only function that calls `.record(` on the audit port, pinned by a source-scanning test; `audit_fields` is a total `match` with no wildcard, so a new variant fails to compile |
| The trace file cannot delay a turn | the JSON layer's writer is `tracing_appender::non_blocking` in lossy mode; gateway test replaces the file with a pipe nobody reads and asserts `conversation.send` latency is unchanged and the dropped-line counter rises |
| The trace file set is bounded during a run | rolling appender with `max_log_files(24)`; test writes across three simulated rotations and asserts the oldest file is removed |
| Every `nessa.execution` open has one close | lifecycle test over the ordering matrix with barriers and a paused clock; the close is `ActiveInvocation::drop`, the same owner that already finishes the execution |
| `error.type` is typed | `error_kind(&AgentError)` total `match` |
| No OTLP endpoint means no socket | `TelemetryConfig::from_environment` returns `Otlp(None)`; gateway test asserts no OTel layer is installed and no connection is attempted |
| Telemetry is not a product input | no code outside `examples/`, tests and composition reads the trace files; structural absence test in the server crate |
| An unreachable OTLP endpoint does not delay a turn | `tracing-opentelemetry` batch export is off the request path; gateway test against a refused port asserts `conversation.send` latency unchanged |
| `gateway.log` stays text | the text layer is unconditional; `from_environment` test |

Unverified and said so in module docs: the OTel batch processor's internal
timer, and whether any harness forwards `_meta` to tool calls.

## Appendix A. What the first draft had, and what removing it costs

| Removed | Replaced by | Cost |
|---|---|---|
| `ExecutionObserver` port + `ExecutionSignal` enum + three total constructors | `tracing` spans and events at the same sites; `audit_fields` keeps the one total match that mattered | Redaction is enforced by a test instead of by a type; embedders consume via a `Layer` instead of a typed enum |
| `nessa-sdk-otel` crate with its own clock, span map, config enum, exporters | `tracing-opentelemetry` in each binary's composition; span map is the lifecycle structs that already exist | None for our binaries; an embedder writes three lines instead of one |
| First-class metrics | Derived from spans and events downstream; `MetricsLayer` if ever needed | A backend or `jq` computes histograms instead of receiving them |
| `RunObserver` port in nessa-mcp | one `info_span!` around the existing bracket | none |
| `HostTelemetry` port on `HostDependencies` | the subscriber is composition, same as the gateway | none |
| Shell `Telemetry` port | deferred until a second consumer exists; gateway span covers the request | UI-side polling latency is not measured in the first release |
| `NESSA_TELEMETRY`, `NESSA_LOG_FORMAT`, `NESSA_TELEMETRY_SAMPLE_RATIO`, `NESSA_OTLP_*` names | the standard `OTEL_EXPORTER_OTLP_*` names only; OTLP is on when an endpoint is set | sampling is always-on until someone needs less; there is no "off" for the bounded local file |
| Slices 3a/3b and 5a/5b | one slice each | none |
