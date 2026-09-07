# Session and stream contracts — proposed design

This is the implementation baseline for proposed ADRs
[0011](../adr/todo/0011-nessa-session-protocol-and-authorities.md) and
[0009](../adr/todo/0009-reusable-event-stream-crate.md). Names below are proposed;
wire schemas, catalogs, Rust APIs, and implementations have not landed.
The current S1 protocol continues to operate unchanged until its negotiated
extension is implemented. [Surfaces and collaboration](surfaces-and-collaboration.md)
defines shared CLI/panel/desktop attachment, scoped credentials, cross-session
messages, sender provenance, and inbox delivery. Those product rules extend the
commands below and do not move into the generic stream crate.
[Sequence diagrams and MCP](collaboration-sequences-and-mcp.md) define an external
MCP adapter using `NessaClient`, including bounded `conversation.get` and
`stream.read_after` queries; native surfaces retain the Nessa wire.

## Ownership and data flow

```text
NessaClient ── connect / catalog / open / commands ──► Gateway
                                                        │
                                              Conversation coordinator
                                                        │
                                                 Binding registry
                                                        │
                                           Rust ACP binding / provider
                                                        │
                                              Rust agent normalizer
                                                        │
Conversation lifecycle ──────────────────────────────────┤
Workflow producer ───────────────────────────────────────┤
Terminal producer + decoder ─────────────────────────────┤
                                                        ▼
                                              Generic event stream
                                                        │
                                              EventStore::append
                                                        │
                                               local commit succeeds
                                                        │
                                              ordered subscriptions
                                                        │
NessaClient ◄── Nessa wire adapter ◄── Gateway ◄───────────┘
```

Storage is an implementation of the stream contract, not a client endpoint.
Each producer chooses a stream; no ordering is promised across streams.
The generic package has no dependency on any box above it or on WebSocket.

## Discovery before agent session creation

1. Establish the normal authenticated Nessa connection. `session.ready` establishes
   authenticated identity and connection state. Agent binding discovery remains
   proposed: a future catalog operation would advertise available bindings.
2. Fetch/refresh the server-owned catalog through `NessaClient`. Each descriptor
   has a stable `bindingId`, agent and integration identifiers, display labels,
   catalog revision, availability, and typed reasons/actions. Agent and
   integration identifiers are extensible strings, not a frontend enum.
3. The UI shows `checking`, `available`, `unavailable`, or `unknown`; only an
   available binding that meets required capabilities enables Open/Send.
   Missing binaries, credentials, compatible versions, or policy permission
   produce explicit setup/error UI. Cached availability is not sufficient after
   reconnect; stale catalog state disables creation until refreshed.
4. `conversation.open` takes `bindingId`, `catalogRevision`, a server-authorized
   workspace reference, and requested configuration/required capabilities.
   The client never submits executable paths, shell commands, or credential env.
   It selects an explicit binding, including any server-advertised default.
5. The server rechecks availability and policy before provider initialization.
   A changed catalog returns `catalog_stale` for refresh. Other typed failures
   include `binding_unavailable`, `authentication_required`, and
   `capability_unavailable`; none is a reason to destroy the Nessa connection.
6. Provider initialization refines effective session capabilities. Open returns
   only when ready, or cleans up partial startup and returns a typed failure.
   Startup has a deadline compatible with the SDK request timeout. A retry uses
   the same command ID to recover the original open operation, including an
   in-progress status; it must not start another provider.
   Required features missing at init fail open; optional features update the UI.

The UI check prevents knowingly unsupported requests. Server checks cover races
and non-UI clients. Discovery may run bounded non-mutating probes, but never a
prompt/tool action or an implicit installation/login. Unknown support requires
an explicit refresh/check; it is never silently treated as available.

Capabilities distinguish actions (`cancel`, `approvals`, `steer`, provider
continuation) from reportable content (plans, file edits, usage). An action is
available only if implemented by the binding, supported by the initialized
provider, and permitted by gateway policy. No union or last-write-wins merge.
Server-side checks remain mandatory on every command. Capability changes after
open are ordered conversation state events. Catalog reasons disclose no secrets.

## Three schemas, one source per contract

| Layer | Required definition |
| --- | --- |
| Generic record | Stream identity, incarnation, cursor, stable event ID, schema identifier/version, opaque bytes |
| Agent payload | Language-neutral JSON schema for the reused `AgentEventPayload` semantics; matching Rust/TS types and golden mappings |
| Nessa protocol | Discovery and commands, lifecycle/approval state, stream subscription and record encoding, typed errors |

The **agent schema must be defined**, not inferred from TypeScript at runtime.
Publish/version that schema with the agent contract in `nessa_ui`, generate its
Rust/TS payload types, and pin an exact version in Nessa. `protocol/` references
that version (a checked, unmodified vendored copy is acceptable); it must not
maintain a separately edited union. Implement a new Rust normalizer against the
shared fixtures. The existing TS package is a reference implementation, not a
Rust runtime dependency. Extending missing semantics happens in that shared
contract, without inventing per-provider UI payloads.

The **Nessa Session Protocol must also be defined** in the existing manifest,
JSON schemas, generated catalogs/types, and fixtures. It remains an extension
of the single native control plane, not a second surface connection or a provider
protocol. The external MCP adapter translates MCP envelopes into typed
`NessaClient` calls; only that client sends the Nessa protocol to the gateway.
Negotiate its supported version/features through connection negotiation; old
clients must not receive an incompatible extended hello without negotiation.
The initial delivery includes mixed-version fixtures and explicit defaults for
omitted optional fields. Do not rely on today's closed JSON schemas accepting
new properties automatically.

Nessa stream records carry `streamId`, `cursor`, `eventId`, schema/version, and
payload. A JSON cursor encodes its incarnation and integer offset losslessly
(e.g. an opaque token containing a decimal offset); JS consumers must not coerce
an unbounded integer to `number`. The current envelope `seq` remains per socket;
`stateVersion` is not repurposed as a replay cursor. SDK subscriptions must expose
the whole committed record, not discard its cursor as today's `onEvent` does.

Agent transcript content adds Nessa-owned `conversationId`, `turnId`, and
normalized agent path where applicable. Collaboration messages exist before a
turn and therefore have no required `turnId`; a later input-assignment event
associates them with an execution. Sender metadata follows the collaboration
contract and is preserved separately from provider normalization. Provider IDs remain internal correlation
metadata. The mapper's own `id`, `seq`, and provider `sessionId` do not define
record identity. Raw provider objects stay in bounded server diagnostics by
default. Tool data that is part of the reviewed payload schema remains payload.
The client transcript adapter reuses the existing fold vocabulary, adapting
record identity/order as needed and deduplicating before applying deltas.

Unknown optional display events may render an unsupported row while preserving
the record/cursor. Unknown required lifecycle or approval semantics cause an
explicit compatibility failure and disable affected actions. Persisted unknown
payloads remain replayable as bytes. Invalid known payloads cause a typed stream
failure; do not silently skip them or advance the projection checkpoint.

## Identity, commands, and turn lifecycle

- A **connection** is one authenticated socket. It is disposable.
- A **conversation** is Nessa's durable product identity, with one primary
  stream. UI tab IDs and provider session IDs are separate.
- A **turn** is a server-owned execution attempt with a stable `turnId`.
- A **provider session** is a binding-owned execution handle. Continuing it is
  independent of replaying Nessa history.
- An **event stream** is generic; terminals/workflows need no conversation ID.

Proposed methods: `bindings.list`, `conversation.open`, `conversation.attach`,
`turn.prompt`, `turn.cancel`, `approval.respond`, `stream.subscribe`, and
`stream.unsubscribe`. Collaboration adds `conversation.list`,
`conversation.message`, and `message.status`, plus owner credential issuance and
revocation operations. Their scopes and delivery semantics are specified in the
[collaboration contract](surfaces-and-collaboration.md). Steering is deferred
until a binding proves its semantics.
Attach locates an authorized existing conversation and its stream; subscribe
replays it. Neither implicitly starts a provider or repeats a prompt. The next
prompt may continue a saved provider session only when the binding can prove
that support; otherwise return `provider_resume_unavailable`, never silently
start a fresh context under the old conversation.

Every mutating command carries a client-generated stable `commandId`, distinct
from the socket RPC correlation ID. Scope deduplication to authenticated principal
and operation target (open is principal-scoped). Same ID and canonical input return
the original acceptance/result; a changed input returns `idempotency_conflict`.
Persist receipts and accepted intent before effects. For v1, make a single
command-accepted record contain the canonical command, allocated identities, and
acceptance response; derive the receipt index from that record on recovery. Do
not acknowledge a separate receipt before the accepted intent commits. Open uses
a principal-scoped control stream so deduplication precedes conversation allocation.
Event-append deduplication alone does not implement command deduplication.

`turn.prompt` returns acceptance with `turnId` promptly; it does not hold an RPC
open for the model's entire run. Exactly one active turn per conversation in v1;
another prompt gets `turn_busy`. Accepted → running → waiting for approval /
cancelling → one terminal outcome (`completed`, `cancelled`, `failed`, or
`interrupted`). Approval resolution returns a waiting turn to running unless
cancellation already won. The coordinator owns these transitions and serializes
races; late provider events cannot reopen a terminal turn. Bindings report
provider outcomes but do not infer command acceptance from provider init lines.

Cancel is idempotent and returns acknowledgement of the cancellation request;
only a terminal event confirms completion. A completed turn may win the race.
Pending approvals are retired on cancellation; the binding cancels descendants
within its ownership, then kills an unresponsive process tree after a bounded
grace period. First binding v1 has no independently detached delegated work.
Closing a tab/unsubscribing/disconnecting does not cancel a turn; explicit Stop
does. Bounded provider execution and approval deadlines prevent abandoned work
from running indefinitely.

On server restart, replay committed state and command receipts. Any accepted or
running attempt whose provider outcome cannot be established becomes interrupted;
never automatically rerun an uncertain tool action. Commit that reconciliation
before accepting further prompts. A retry of its command ID returns the same
attempt, not a new execution. A new turn requires a new command. This is not
exactly-once execution across a crash and an external side effect.

## Approval and execution ownership

An authoritative pending approval includes `approvalId`, conversation/turn ID,
operation preview, deadline, and offered normalized choices (`choiceId`, label,
allow/deny action and once/session scope). The binding privately maps each choice
to an offered provider option; it cannot manufacture an unoffered permission.
The first supported bindings must supply representable choices; otherwise fail
explicitly or cancel the request. Approval events may decorate the transcript,
but pending state and replies do not depend on transcript rows or raw payloads.

`approval.respond` selects a choice with a command ID. Validate owner, pending
status, turn, and offered choice. Persist the decision before forwarding it.
Identical retries return its receipt; conflicting, expired, or stale decisions
are typed failures. Multi-client races have one authoritative winner. Denial
and turn cancellation are different outcomes. Timeout/cancel resolves the
provider callback and emits a resolved state; reconnect restores the pending
set by replay. No new controls are enabled until replay catches up. After a
crash, uncertain forwarding is reconciled with the interrupted turn rather than
replayed as an unsolicited authorization.

The binding must convert SDK callbacks into this contract when an SDK binding
is added; the Claude SDK output mapper alone cannot expose approvals. Rust ACP
is the initial implementation choice, with its exact supported protocol version
pinned by fixtures before coding. Only advertise ACP host filesystem/terminal
services actually implemented with gateway workspace and access policy checks.

The gateway owns workspace authorization, command/stream access checks, binding
selection, secret lookup, process supervision and cleanup. The provider binding
owns supported protocol correlation and provider-specific host integration, not
the external harness execution loop. [ADR 0012](../adr/todo/0012-agent-harnesses-and-optional-tools.md)
keeps external harnesses unmodified and defines MCP/CLI tools for both them and
Nessa’s own internal agent. No arbitrary executable
or secret from a client intent; no raw credentials in discovery, event records,
or diagnostics. First implementations must bound startup, pending requests,
provider frames, outbound frames, and buffers; oversized content produces a
typed failure until an explicit attachment/chunking contract exists.

## Generic crate contract

The [event-stream library](https://github.com/nessalabs/event-stream) exists outside
this repository; its Nessa integration and verification remain in ADR 0009. The following are Nessa
consumer requirements and conceptual APIs to reconcile with that external crate,
not instructions to implement another stream runtime in this workspace.

Conceptual API responsibilities (not final Rust signatures):

| Operation/seam | Contract |
| --- | --- |
| `append(stream, eventId, schema, bytes)` | Atomic deduplication and committed cursor allocation; returns original record on an identical retry |
| `subscribe(stream, after)` | Historical records strictly after cursor followed by live records in the same order; cancellable bounded subscription |
| `read_after(stream, after, limit)` | Bounded historical page; reports unavailable history rather than skipping |
| `bounds(stream)` | Incarnation, earliest resumable cursor and latest committed cursor |
| `EventStore` | Injected storage transaction/recovery/ownership implementation, with the same contract suite for every adapter |
| Incremental decoder | Accept arbitrary byte chunks, produce zero/many decoded items, finalize EOF, return typed malformed/truncated/oversized-input failures |

The runtime serializes appends per stream. The store atomically checks event ID
and input equality, allocates the next offset, and commits the record before
success. Retry of a committed identical event returns its original cursor and
does not notify again; differing schema/bytes under that ID fail. The adapter
must not implement this as separate “check, increment, insert” transactions.
V1 keeps event ID/receipt history for each stream's lifetime, with no automatic
compaction. Later retention must define a deduplication horizon explicitly.

Records are immutable and totally ordered only within a stream incarnation.
Recreating/resetting a stream creates a new incarnation; old cursors cannot
accidentally address new data. Default subscription starts at the beginning;
a current tail cursor is an explicit opt-in when full history is unnecessary.
Wrong incarnation, ahead-of-store cursor, and unavailable history are distinct
errors containing applicable bounds. A failed commit has no published record.
An acknowledgement lost after commit can be retried with the same event ID.
The producer must retain that ID for retries; the crate cannot deduplicate a
source that regenerates a new ID for the same observation.

For replay/live correctness, the runtime registers the subscriber and captures
a committed high-water mark under the same per-stream serialization boundary.
It replays through that mark and then delivers commits above it, never interleaved.
Wakeups may coalesce; the store remains the record source. A race cannot create
a missing record. One uninterrupted subscription emits each record once;
network reconnect may repeat applied records, so consumers deduplicate by
stream/cursor or event ID and advance checkpoints only after successful apply.
A persisted checkpoint is valid only alongside its matching persisted projection;
an in-memory UI reload must replay from the beginning in v1.

Queues, replay pages, and decoder buffers have explicit bounds. Slow consumers
are disconnected with `subscriber_lagged` and their last delivered cursor; they
resubscribe from their own last applied checkpoint. Dropping a subscription
releases its resources without stopping a producer. Storage errors never fall
back to publishing uncommitted events. Nessa stops new work, backpressures the
provider where possible, and cancels it if bounded buffering cannot protect the
stream. An out-of-band connection fault may report storage failure, but cannot
pretend to be a committed terminal event; reconciliation is persisted on recovery.

Generic parsing/framing adapters are optional modules. They handle split UTF-8,
partial frames, multiple frames per chunk, configured size bounds and EOF.
They do not own provider session lifecycle, terminal emulation, or normalization.
Already-decoded input bypasses them. Parser failure policy belongs to the producer
(fail, diagnostic, or explicitly modeled skip); it is never silently swallowed
by the stream. Crash recovery covers committed parsed records only. Reconstructing
partially consumed input requires source offsets/checkpoints or a raw input log,
which v1 does not promise. A non-replayable source cannot gain lossless crash
recovery merely by attaching this crate.

Local storage is mandatory for Nessa's durable configuration, but optional for
external crate consumers. An ephemeral adapter must identify itself as such.
Nessa paths follow stage/instance isolation; losing internet must not prevent a
local commit, although a cloud-dependent provider can still stop generating.
A future replica reads local committed records with stable origin identities.
Snapshots, retention, replication and multiple writers are deferred. For v1,
`history_unavailable` shows explicit recovery UI; there is no fictional snapshot
fallback. Retention must not be enabled until state restoration is designed.

## Implementation gates and open implementation choices

Local credential issuance, persistence, authentication/authorization, and
revocation are implemented under ADR 0010. Auth API readiness and operating bounds belong to
ADR 0007. ADR 0008 owns discovery/preflight, payload normalization, and agent
execution; ADR 0012 owns optional MCP/CLI packaging. ADR 0011 owns shared
attachment, transcript subscriptions, and collaboration. These scopes integrate
the existing external event-stream library through ADR 0009.

The remaining integration order is:

1. Pin a reviewed revision of `nessalabs/event-stream` and inspect its public
   contract (the current manifest is `event-stream` 0.1.0, unpublished). Verify its guarantees against the consumer requirements above;
   resolve differences with the external crate before integrating it. Storage
   selection and implementation remain in that separate project.
2. Define the agent payload schema with TS/Rust generation and shared fixtures,
   and the Nessa method/event schemas with `protocol:generate` / `protocol:check`.
3. Consume the external crate and its chosen local adapter. Verify ordering,
   durability, decoding, and recovery at the Nessa integration boundary; do not
   duplicate its core or store implementations here.
4. Implement discovery, coordinator, Rust normalizer, and one Rust ACP binding
   through the CLI and panel. Replace echo and prove approval/cancel/reconnect,
   shared surface attachment, and attributed local session/external messaging.
   Scoped principal authentication must land before enabling collaboration.
5. Add a second agent on ACP to demonstrate reuse. Add another integration only
   when its execution boundary and actual capabilities have been specified.

Acceptance scenarios must cover: catalog unavailable/stale/init mismatch; old
client compatibility; normal prompt/stream/terminal outcome; retry after lost
acceptance; competing prompts; approval conflict/timeout/cancel/reconnect;
provider crash; server crash before/after commit and uncertain external effects;
concurrent appenders; duplicate/conflicting event IDs; replay during heavy writes;
wrong/ahead/expired cursor; split/truncated/oversized input; disk-full/write
failure; slow subscriber; reload without a projection checkpoint; stage isolation;
and unauthorized command/subscription. Verify one persisted replay yields the
same projection as live delivery, including deltas and pending approvals.

Also run the [collaboration acceptance scenarios](surfaces-and-collaboration.md#acceptance-scenarios),
including token isolation, sender attribution, and multi-surface/inbox races.
These are implementation gates, not a claim that current code passes them.
