# Execution sessions and lifecycle

## Lifecycle terms

| Term | Meaning in ACP | Current Nessa SDK behavior |
| --- | --- | --- |
| Create a session | `session/new` creates an agent conversation context. | `Agent::prepare(provider, manager, audit)` restores durable local evidence without provider I/O; an attributed attachment authorization starts provider opening separately. |
| Run an execution / prompt turn | `session/prompt` runs a user message within that context. | `Agent::invoke()`; normal completion leaves the session available for another execution. |
| Queue / steer input | Provider-specific controls may extend prompt delivery. | `enqueue` and `enqueue_steering` wait for invocation boundaries; `steer` uses supported native injection. See [scheduling](scheduling.md). |
| Withdraw waiting input | No provider effect is needed for locally pending input. | `remove_queued` retains withdrawal evidence; active/injected input cannot be unsent. |
| Cancel a turn | `session/cancel` interrupts ongoing work. | Sent internally during shutdown. There is no public cancel-only operation that preserves the live handle yet. |
| Close an active session | Capability-gated `session/close` cancels work and releases that session's resources. | `Agent::close(action)` cancels SDK waiting inputs with attribution and performs local binding shutdown: it sends `session/cancel`, then tears down the owned process. It does not send `session/close`. |
| Load / resume a session | Capability-gated `session/load` restores context and replays history; `session/resume` restores context without replay. | `Agent::invoke()` automatically resumes a closed context through `session/resume`; unsupported resume or missing history is an error. `AgentProvider::open(None)` explicitly creates a new context at the adapter boundary. |
| Delete a session | Remove stored session data/history according to the provider's deletion contract. | No deletion operation is implemented. Closing does not request history deletion. |

`ExecutionSession::is_closed()` means this particular live session has permanently
stopped accepting executions, tool observations, and permission answers. It becomes true when shutdown
starts, before process cleanup finishes. It does not mean a stored conversation
was deleted. The backend restores the provider context before beginning an execution on a fresh
aggregate. The old aggregate stays closed; its guard is still an invariant.

ACP protocol capabilities and this binding's implemented methods are distinct.
See [ACP session setup](https://agentclientprotocol.com/protocol/v1/session-setup)
and the separate [session deletion specification](https://agentclientprotocol.com/rfds/session-delete).

## Execution sessions and shared ACP infrastructure

`domain::agent_execution::sessions::ExecutionSession` owns one opened execution
context: its `ExecutionSessionId`, admission state, active `ExecutionId`, observed
`ToolCall` entities, and pending `PermissionRequest` entities. It admits one
execution at a time, isolates tool observations, rejects stale answers, and cancels
pending requests when an execution finishes or the session closes. Closing is
permanent; process cleanup and final result delivery remain infrastructure effects.
The aggregate reuses the existing entity merge/answer rules.

```text
open --> idle --> executing --> idle       (sequential executions)
          |          |
          +----------+--> closing --> closed

same Agent -- next invoke --> resume provider context
                                      --> fresh live aggregate --> executing
```

The first diagram is one live aggregate; the second is the client restoring a
provider context into a new aggregate. Arrows show allowed lifecycle transitions. Closing rejects new executions and
permission answers and tool observations immediately, cancels pending permissions, and waits for process
cleanup. The old live aggregate stays closed; saved provider history is not deleted.
The session client automatically restores that context for its next execution after cleanup succeeds.
Execution completion and process cleanup
have separate outcomes; receiving a final message does not establish cleanup.

### State lifetimes

| State | Owner and lifetime | Cleared/changed by |
| --- | --- | --- |
| Provider context/history | Provider, identified by the ACP session ID; persistence depends on that provider. | Explicit provider lifecycle operations. Local close does not request deletion. |
| `ExecutionSession::is_closed` | One live domain aggregate; false initially, true from the start of its shutdown. | `close()` permanently marks that instance. Successful restoration constructs a fresh open aggregate with the same provider context ID. |
| `active_execution` | Aggregate; one execution including its cleanup. | `begin_execution` sets it; `finish_execution` clears it. `close` alone does not clear it. |
| `seen_execution_ids` | Aggregate; every execution ID admitted by this live attachment. | Retained through finish and close until aggregate drop. IDs cannot be reused; a rejected begin does not reserve one. |
| `tools` | Aggregate; latest tool observations in that execution, frozen when the attachment closes. | Execution finish clears them. They do not carry across execution/restoration boundaries. |
| `permissions` | Aggregate; pending reviews only. | Answer/cancellation removes a review and returns its resolution evidence. Close/finish cancels remaining reviews. |
| `permission_ids` | Aggregate; every admitted ID in that execution, pending or resolved. | Execution finish clears the set. Earlier cancellation must not release an ID for reuse. |
| Original review input and retention accounting | Application controller; pending review lifetime. | Resolution pairs input with the returned evidence, then releases its charge. |
| Permission ID allocator | Stable ACP session client, across process restarts. | Never resets on resume, so an old answer cannot match a new review even if a caller repeats an execution ID. |
| RPC IDs, worker tasks, subprocess ownership | ACP/process infrastructure; one live connection. | Verified teardown; a resumed connection gets fresh wire state. |
| Session closure and permission evidence | Application-owned audit port and injected storage adapter. | Not cleared by domain execution/session cleanup. Retention follows the adapter's explicit contract. |

The live aggregate retains execution identities so a delayed callback cannot gain
authority over a new run that reuses an old ID. The domain constructor limits each execution ID to
256 UTF-8 bytes; retained identity history grows linearly with admitted executions and
has no arbitrary session-length cap. This ephemeral history is not restored
provider history. The Agent's persisted admission records separately protect
logical submission identity across restoration.

Provider context IDs (`ExecutionSessionId`) are nonblank, at most 256 UTF-8 bytes,
and stored compactly before being copied into evidence. Local storage keys
(`SessionId`) have a separate 128-byte portable ASCII contract and compact storage.
Rejecting an invalid provider ID does not establish a valid domain context for audit.

Closing an aggregate and finishing an execution are separate transitions. A finish
with `SessionClosed`, `SessionFailed`, `DeadlineExceeded`, `EventConsumerDropped`, or
`SessionHandlesDropped` requires a preceding close, whose evidence captures the
active execution ID. An open aggregate rejects those causes without releasing its
execution, tools, or review accounting. Execution failure may settle an open run;
a deadline alone cannot release authority while provider work may still continue. The closed flag rejects new work and tool observations immediately. Retained execution
identity and existing tool observations remain available for cleanup correlation;
late updates cannot change them. `close` validates its lifecycle cause before any
mutation. `close_execution(expected_id, reason)` checks the captured execution ID before any
mutation, including repeated calls. Execution-owned failure and deadline callbacks
use this method so a delayed callback cannot close a later execution. Session-wide
`close(reason)` rejects `ExecutionFailed`;
`SessionFailed` describes attachment failure, including startup or idle failure
without an execution. A repeated session-wide close preserves its original evidence even
after execution settlement; execution-scoped close still requires its matching active ID. Deadlines may belong to startup, protocol control, or
execution and do not require an active run. Session closure, attachment failure,
correlated execution failure, deadlines, and dropped consumers/handles are valid causes; provider review withdrawal, normal
execution completion, and custom permission reasons do not close a session.
Bulk permission cancellation separately requires the intended execution ID so a
delayed callback cannot drain reviews from a later execution. Automatic restore is an infrastructure operation
coordinated by the application backend; it must complete before the new aggregate
begins an execution. It never revives old permissions, observations, or RPC IDs.

ACP can fail while idle. If that generation's ordinary protocol/provider failure
has not reached an operation or event reader, the next preparation returns it
before starting another process or sending a prompt. After that failure has been
reported and cleanup is confirmed, a later explicit invocation may resume the
same context; an additional close is not required. Audit or uncertain-cleanup
failures still block restoration. The old event reader drains retained observations
with their original execution identities, but retirement prevents its already
reported failure from being attached to a newly dispatched invocation.

A restoring worker reserves its reader slot before starting, but joins the public
reader only after startup succeeds. Failed startup releases that slot; repeated
failures do not require the caller to drain a reader before retrying. Dropping a
preparation waiter keeps the worker and reserved slot owned by the session, so a
later attempt waits for the same startup. Cleanup and audit failures still prevent
reuse even though a failed startup's unused reader slot has been released.

Tool notifications already in flight can arrive after `session/cancel`. During
teardown ACP drains those notifications without changing the closed aggregate,
while continuing to process the provider's terminal response. Session closure
keeps its original reason/initiator; execution settlement and cleanup retain their
independent outcomes. A provider-reported cancellation closes the attachment with
runtime `SessionClosed` evidence and records the execution as `Cancelled` after
cleanup succeeds.

A retiring ACP generation seals command admission as soon as its drive loop ends,
before final audit delivery or cleanup can suspend it. Completion is published
before waking the final execution waiter. A queued follow-up therefore either
waits for that cleanup and restores the context, or receives its actual failure;
it cannot enter a receiver that has stopped processing commands.

These session concepts have different owners:

| Concept | Meaning |
| --- | --- |
| `ExecutionSession` / `ExecutionSessionId` | An opened agent context, qualified by its binding instance; exposed through `ProviderSession::id()`. The ACP adapter maps the provider's opaque session ID into this value. |
| Application `Agent` / `SessionManager` | Public invocation/control entry point and its local snapshot manager. See [Agent](agent.md). |
| `ProviderSession` / `ProviderSessionBackend` | Internal provider context handle and adapter control port. |
| `PermissionSessionId` / `PermissionApplicationId` | Host-owned boundaries for scoped permission intent; a provider ID cannot create one or grant authority. |
| `nessa-auth::AuthenticatedSession` / `AuthContext` | Verified credential/membership selectors. These are not agent contexts or approval action attribution. |

`ActionContext` remains a narrow application attribution value: actor, surface,
and action request. Reusing `AuthContext` there would introduce credential and
membership state without supplying the surface/action identity it needs. The host
maps verified identity into action attribution. Similarly, `PromptText` now lives
beside `SystemPrompt` and its contributions; streamed `MessageChunk` remains separate.

## SDK scheduling and storage boundaries

The live aggregate's `Busy` guard does not reject queued admission. Agent keeps
waiting work outside that aggregate and dispatches one invocation at a time.
Ordinary input is FIFO; boundary steering has priority without interrupting the
current invocation. Native steering is separately acknowledged against that
invocation. Removing waiting work does not remove its saved input or audit record.

Agent close starts provider cleanup independently and joins invocation settlement.
Direct and queued invocation work is supervised by Agent after admission starts;
dropped waiters do not stop settlement or persistence. Close retains
the storage lease, so another manager cannot overwrite the same conversation while
this Agent may resume it. Initialization failure and Agent drop retain a protective
lease until the attachment confirms cleanup; initialization errors expose explicit
cleanup retry. Dropped queued receipts do not cancel work; runtime or
process shutdown is not automatic durable-job recovery. See [scheduling](scheduling.md)
and [Agent storage](agent.md#session-manager-and-storage) for the detailed contract.

A live aggregate retains its first local closure evidence through settlement.
Closure and later provider settlement describe distinct facts for every valid
closure cause: a late success, refusal, cancellation, or failure retains its actual
result in `ExecutionFinish`, while `SessionClosure` and its cancelled reviews keep
the original cause. This includes completion racing a deadline or failure closure.
Attachment-terminal failures and deadlines still require a preceding close;
permission-only causes are never execution failures. Settlement releases retained
execution state but never reopens the closed aggregate or confirms OS cleanup.
The application owns external cleanup and admission of a fresh attachment; cleanup
failures remain separately reported effects.

## Application lifecycle ownership

The private `SessionLifecycle` coordinates admission, active dispatch, and
shutdown for direct, queued, native-steered, and provider-control work. A work
permit belongs to the supervised operation until its evidence and response settle;
dropping the caller's waiter cannot release that ownership. Physical cleanup and
settlement of accepted work are separate completion boundaries.

```text
Agent API -> supervised work permit -> provider operation
                    |                       |
                    v                       v
          SessionLifecycle <--- provider session state / cleanup report
                    |
                    +--> SessionManager -> InvocationHistory -> saved projection
```

Arrows show coordination and evidence flow. The lifecycle owner distinguishes an
work generation from a provider generation: cancelling newly queued
work does not invent a new provider process. A delayed result affects the work generation
that accepted it. Provider restoration creates a new physical generation only
when the previous attachment has been cleaned up.

`CleanupReport` records physical resource status independently from audit delivery
and an associated operation failure. `ProviderExecutionReply` distinguishes
rejection before dispatch from settlement after the adapter accepted ownership.
`ExecutionReport` preserves an actual provider result separately from later
SDK failure or local cancellation without a provider response. These facts are
persisted explicitly; `AgentError` is a diagnostic projection, not a lifecycle
state machine.

This does not turn snapshots into a complete permission audit log or durable job
queue. The storage port receives a complete snapshot; local file storage appends
changed metadata and event tails as JSONL. Full snapshot copying and validation
still depend on conversation length, while streaming text does not trigger a save
or a full-history scan for every chunk. Per-invocation output limits and transport queue bounds
protect different retained data; neither is a total conversation-size limit.

Journal decoding first walks each complete line without building owned records.
It bounds field names, decoded strings, collection structure and nested diagnostics;
raw string tokens are limited to six times the selected decoded-byte allowance so
JSON Unicode escapes remain valid. Each changed invocation has a 160 MiB decoding
allowance for text and structural slots. Shared retention validation then enforces
the exact live payload and capacity limits. This is not a total-history or process
memory cap: one checkpoint may contain many valid prior invocations. Incomplete
final lines are scanned without decoding or allocating their payloads.
Known tool content, location and permission-option arrays are also limited by
how many domain element slots fit within the live 32 MiB payload allowance.
Unrecognized shapes have finite fallback string, object, array and recursion
bounds and are subsequently rejected by the typed schema.
