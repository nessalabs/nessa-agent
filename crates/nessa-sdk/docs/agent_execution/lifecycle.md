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

Attachment audit records are immutable outputs of `SessionLifecycle`, exposed
through the existing `AttachmentAuditRecord` getters. Start, context publication,
failure, abandonment and close derive their related facts under the lifecycle
lock; the public record has no constructor or writable fields. The two record
compile-fail doctests hold that boundary. `authorization_wait_and_start_publish_one_coherent_status`,
`close_joins_failed_transition_registered_before_delivery` and
`abandoned_authorization_blocks_replacement_until_audit_settles` check the issued
session, generation, stage, cause and actor together.

| Ordering | Required evidence and effect | Regression |
| --- | --- | --- |
| Start synchronously selects Starting; close seals before the attachment worker first runs | Join the real Started and Closed audit acknowledgements with their original actors; no provider acquisition; the attachment waiter is Closed, while close succeeds only after its actual cleanup and evidence complete | `close_before_the_attachment_worker_joins_its_actual_started_audit` |
| The same ordering, but Started audit is rejected or its acknowledgement is held | Preserve the actual sink failure, or keep close pending until the real acknowledgement; never substitute the worker's generation refusal for an audit receipt | `close_before_the_attachment_worker_keeps_real_started_rejection_and_late_ack` |

The installed Started obligation is already owned when close seals admission.
Its evidence uses the existing serialized proof-retirement/audit owner; the
attachment generation fence still guards provider I/O. A failed proof retirement
prevents both the audit effect and provider acquisition and remains a failure.

An audit sink's diagnostic does not replace the known origin of a failed
attachment audit. `AttachmentAttemptFailure` retains the transition source;
`starting_audit_origin_does_not_depend_on_sink_error_variant` and
`published_audit_origin_does_not_depend_on_sink_error_variant` cross start and
publication with protocol and storage diagnostics. These failures retain the
Audit category and original diagnostic, prevent dispatch and complete owned
cleanup without adding a provider-failure record. Context publication remains
separate from audit acknowledgement and dispatch readiness.

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

### Live approval control orderings

`Agent::set_approval_mode` uses the existing `SessionLifecycle` control runner.
Its owned task holds the scheduler lock and work permit through acknowledgement
and generation-correlated publication, independently of the caller's waiter.
The backend owns verified native application; the SDK owns admission's mode.
The SDK records attributed native configuration intent, verified acknowledgement,
and settlement through its audit port. Queue admission separately records the
configuration captured for that input; the current configuration fact owns restore.
The host's metadata journal is a separate commit boundary.
`agents::approval` checks the approval-specific orderings. The common runner's
`agents::coordination::ready_results` and
`control_admitted_before_provider_reply_cannot_first_poll_after_unusable_state`
check ready acknowledgement and first-poll exclusion.

| Ordering | Result |
| --- | --- |
| Generation stopped before its backend's first poll | No provider command starts; control reports Closed with cleanup required. |
| Started control has acknowledgement and stop ready together | The already-ready acknowledgement retains its meaning; generation correlation prevents authorizing a replacement attachment. |
| Pending native response, then close with confirmed cleanup | Control stops and releases its scheduler ownership, allowing close to settle without the response. |
| Accepted command, then dropped caller, then verified response | Owned control publishes the applied mode before releasing scheduler ownership; later admission records that mode. |
| Caller dropped while response pending, then close | Physical cleanup and close settle without that response; a late answer cannot authorize the detached generation. |
| Response verified before close | Applied mode belongs to the old attachment; a new attachment uses the binding preset. |
| Failed/uncertain native response, including a backend panic before constructing its future | Existing control runner fences the affected generation; its mode is not published as verified. |
| Repeated successful changes on the same attachment | Admissions observe the most recently acknowledged mode, under the same scheduler exclusion. |

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

When an owned lifetime is installed, the lifecycle reads its seal after the tree
admission scope and before taking the lifecycle lock. A sealed lifetime refuses
new attachment and queued work. `maybe_reopen` returns without taking the tree
lock once the seal is set. Disposal (`SessionHandlesDropped`) seals before the
lifecycle lock and keeps an earlier attachment-stop cause. The agent's own
attachment cleanup is reported back through `note_attachment` after the stop
completes; that report does not by itself close descendants.

`CleanupReport` records physical resource status independently from audit delivery
and an associated operation failure. `ProviderExecutionReply` distinguishes
rejection before dispatch from settlement after the adapter accepted ownership.
`ExecutionReport` preserves an actual provider result separately from later
SDK failure or local cancellation without a provider response. These facts are
persisted explicitly; `AgentError` is a diagnostic projection, not a lifecycle
state machine.

This does not turn snapshots into a complete permission audit log or durable job
queue. The storage port receives only the semantic changes since the last save:
they are the one account of what changed. The session applies each change to its
own state the same way storage does, so neither keeps or compares a copy of the
other's. Record storage checks the changes and appends one fact at the save
boundary. Streaming text does not trigger a save for every
chunk. Per-invocation output limits and transport queue bounds protect different
retained data; neither is a total conversation-size limit.

Semantic decoding bounds fields, strings, collection structure and nested diagnostics;
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

## Caller wakers

A public future polled on the caller's task registers the caller's `Waker`
with whatever it waits on: a receipt's result channel, the event broadcast, an
attachment's cancellation or stop channel, or a Tokio mutex the Agent's own
tasks also take. The task that publishes or releases (the queue runner, an
invocation, a withdrawal, a close, an admission) then calls that waker
synchronously. The safe `std::task::Wake` trait does not forbid a waker to
panic. Before #431 such a panic unwound into the Agent's task. A queue runner
died with `running` still set, so queued work never dispatched. An invocation
failed with `invocation task panicked` and dropped its attachment. A close
stopped part-way and poisoned the lifecycle lock, so every later lifecycle call
panicked. Tokio also drops the wakers it has not reached yet when one unwinds,
so other tasks waiting on the same receipt could sleep forever.

**Owner.** One primitive, `contain_caller_wake` in
`application/agent_execution/caller_wake.rs`, owns caller-waker faults. A
public wait passes it the caller's waker and its own identity. It polls the
wait with a waker of its own, which calls the caller's waker inside
`catch_unwind`. A panic is logged with the wait's identity and goes no further. A
second panic from dropping the panic payload is also caught; that payload is
leaked rather than dropped. The same owner contains destruction of its cloned
caller waker, which can occur on the publisher's task after notification. Its
`Drop` catches that destructor panic and forgets the fault payload without
dropping it, so a payload destructor cannot escape into the publisher or runtime.
None of the publishers behind the waits in this
table catches a notification panic itself, and nothing resets `running` or
repairs lifecycle state after one.

Every public Agent wait whose caller waker an SDK task wakes polls through
`contain_caller_wake`; the waits that only await caller-supplied provider and
storage ports are listed under "Not covered" below. This includes
`AgentInitializationError::retry_cleanup` (whose cleanup handle is always empty
today, so its wrapper cannot yet be exercised). That includes the operations
that spawn their owner and await its `JoinHandle`: `invoke`, `enqueue`, `enqueue_steering`, `steer`,
`reorder_queued`, `remove_queued`, `close`, `set_approval_mode`, `set_effort_level`,
`answer_permission`, `cancel_permission` and `answer_question`. Tokio wakes a
`JoinHandle` waiter inside its own `catch_unwind`, but it drops the caught
payload outside it. A payload whose drop panics would then escape the task's
completion and abort the process on a multi-thread runtime. A task whose
`JoinHandle` was dropped, such as the attachment task, has the same exposure
one level down: its own panic is stored and dropped inside that catch, so a
payload that panics twice when dropped escapes it.

Tests are in the public `application` test binary, under
`application::agent_execution::agents::review_regressions::caller_wakers`.
Each one runs its journey first with a waker that does not panic, except the
two multi-thread child processes: their clean passes are the sibling tests
named in their rows. The containment warning's wait identity is asserted in
`joined_operation_child`, once per spawn-and-join operation; that assertion
runs only in a child process because tracing caches callsite interest
process-wide, so concurrent tests can hide an event from a thread-local
subscriber.

The owned coordinator rows use tests under
`application::agent_execution::subagents`. The small
`effects::panicking_caller_waker_and_payload_do_not_strand_other_close_waiters`
fixture checks retained close progress, but sender destruction can rescue its
notification when an uncontained wake unwinds; that fixture alone does not
prove containment. The destructor fixture
`effects::panicking_caller_waker_drop_does_not_strand_registered_close_waiters`
keeps the pending public wait alive after releasing the caller's original waker
references. Its counters assert that the last waker destructor runs in the SDK
publisher task and that the fault payload is never dropped, alongside completion
of the healthy waiter and exactly one physical close.

| Public wait | Published or released by | What each owner keeps when the caller's waker panics | Test |
| --- | --- | --- | --- |
| `QueuedInvocation::wait` (and `QueueAdmission::wait`), after normal completion | The queue runner | The stored result, scheduling evidence and slot/work retirement are unchanged. The runner continues to the next queued item | `panicking_receipt_consumer_preserves_independent_queued_work`: the independent tail dispatches (two dispatches) and settles `Completed`, and both stored records end `Settled` |
| Other waiters on the same receipt (same-submission retries) | The queue runner | All sixteen are woken and read the retained result | Same test. Which waiters Tokio would drop without the wrapper depends on its randomly chosen notification bucket, so this assertion detects sibling loss with high probability, not on every run. The tail-dispatch assertion fails on every run |
| The same receipt wait, panicking with a payload whose drop also panics | The queue runner | As above | Same test, third pass |
| `QueuedInvocation::wait`, after withdrawal | `remove_queued`'s task | The withdrawal still returns `Removed` and keeps its `Withdrawn` evidence; the receipt resolves `Closed` | `panicking_receipt_consumer_does_not_fail_its_withdrawal` |
| `AgentEvents::next` | The invocation that publishes the update | The invocation settles `Completed`, the attachment stays attached and later queued work runs. The update stays queued for the next poll | `panicking_event_subscriber_does_not_stop_the_invocation_that_published` |
| `AttachmentCancellation::wait` | `close`, while it holds the lifecycle lock | Close returns normally, the lifecycle lock is not poisoned, and the Agent can attach and invoke again | `panicking_attachment_cancellation_waiter_does_not_interrupt_close` |
| `ProviderOpenControl::wait`, polled by a provider during open | `close`, while it holds the lifecycle lock | Close returns normally and the lifecycle lock is not poisoned | `panicking_provider_open_stop_waiter_does_not_interrupt_close` |
| `Agent::queued_ids`, `Agent::idle_for_approval_change` | Any task releasing the scheduler lock; the test uses an admission | The admission returns its receipt and the input runs | `panicking_lock_waiter_does_not_fail_the_admission_that_released_it`, one case each |
| `SessionManager::snapshot` | Any save releasing the evidence lock; the test uses an admission | As above | Same test, `CommittedSnapshot` case |
| `AttachmentWait::wait`, plain panic | The attachment task, as its last action, holding no lock | The attachment is attached and runs work | `panicking_attachment_waiter_leaves_the_attachment_attached`, which also passes without the wrapper |
| `AttachmentWait::wait`, panic whose payload panics twice when dropped, on a multi-thread runtime | The attachment task, whose `JoinHandle` the Agent does not keep | The runtime keeps running; the attachment is attached and runs work | `twice_panicking_payload_attachment_waiter_does_not_abort_the_runtime`, which runs `triple_fault_attachment_waiter_child` in a child process. Without the wrapper the child aborts with SIGABRT |
| `close` and the other spawn-and-join operations, plain panic | Tokio's task completion, inside its own `catch_unwind` | Close completes; the Agent can attach and invoke again | `panicking_close_waiter_does_not_interrupt_close`, which also passes without the wrapper |
| Each spawn-and-join operation, panic whose payload drop also panics | Tokio's task completion, which drops the payload outside its catch | The panic stays in the wrapper, which logs the operation's own wait identity, and the operation completes. No warning is logged on the clean pass | `panicking_payload_waiter_of_each_joined_operation_is_contained`, one child process per operation running `joined_operation_child` on a current-thread runtime, where the owner cannot run before the first poll. Without an operation's wrapper its case fails by name: the payload's drop panic unwinds out of `block_on` |
| `close`, panic whose payload drop also panics, on a multi-thread runtime | Tokio's task completion, which drops the payload outside its catch | The runtime keeps running; close completes and the Agent can attach and invoke again | `panicking_payload_close_waiter_does_not_abort_the_runtime`, which runs `double_fault_close_waiter_child` in a child process so an abort fails the test. Without the wrapper the child aborts with SIGABRT |
| `OwnershipCoordinator::spawn` | The coordinator's admitted spawn worker, via its retained result watch | Caller loss leaves the admitted worker running. Its terminal result remains cached for joined and identical retries; a factory poll fault returns `OwnershipFailure::Startup(PortFailure::Uncertain)` without a second factory call | `subagents::supervision::dropped_spawn_caller_and_two_joiners_receive_one_cached_factory_failure` (caller loss and immutable cache); the common waker destructor boundary is exercised by the close fixture below |
| `OwnershipCoordinator::end_lifetime` | The selected close-generation worker, via its result watch | Cleanup and publication continue independently of the caller. Joined waiters keep that generation's typed result: `OwnershipFailure::Audit(PortFailure::Rejected)` remains their result after a later explicit retry succeeds, without physically closing a released owner again | `subagents::settlement::joined_close_waiters_keep_failed_generation_after_successful_retry_and_caller_loss`; `subagents::effects::panicking_caller_waker_and_payload_do_not_strand_other_close_waiters`; `subagents::effects::panicking_caller_waker_drop_does_not_strand_registered_close_waiters` |
| SDK-issued `OwnedLifetime::join_descendants` | The selected ownership close-generation publisher | The common caller-waker boundary contains last-waker destruction and forgets its fault payload. Observation and Completion publication finish; healthy joins receive the retained result | `subagents::gate_waiters::panicking_gate_join_waker_drop_is_contained_in_owned_publisher` |
| `McpServers::open`, `call_tool`, `read_app_resource`; `McpSession::list_tools`, `read_ui_resource` | The connection's reader task, via a oneshot, or `stop`'s watch while it holds the live-session lock | The reader keeps serving the next call. `stop` returns and a later open is `Stopped`; the live-session lock is not poisoned | `caller_wakes` in `tests/infrastructure/mcp/` |
| `McpSession::serve` | The connection's reader task, via a oneshot, and then `close`'s process reaper (`child.wait`) when the harness ends | The reader keeps serving a later call on that connection | `panicking_serve_waiter_leaves_the_reader_serving` |
| `McpSession::close` | The process reaper, via `child.wait` inside `ServerProcess::stop` | A second session's close still finishes | `panicking_close_waiter_does_not_stop_the_process_reaper` |
| `McpServers::stop` | Tokio's task completion for the close tasks it joins | `stop` returns. Tokio catches a plain `JoinHandle` wake panic and drops the payload outside that catch; a payload whose drop panics stays in the wrapper | `panicking_payload_stop_waiter_does_not_abort_the_runtime`, which runs `double_fault_stop_waiter_child` in a child process. Without the wrapper the child aborts with SIGABRT |
| `ProcessScope::cleanup` | The timer while `wait_scope` sleeps, then the process reaper on `child.wait` | A second process still cleans up | `panicking_cleanup_waiter_does_not_stop_later_cleanup` |
| `RecordStorage::runtime` (first use: `initialize`, and `open`, `open_existing`, `read_committed`, `record_identity`, `record_source`, `record_source_expected` when nothing has opened it yet) | The SQLite worker thread, via the open oneshot (`ready.send`), before the worker loop | The worker keeps answering a later read. One wrap inside `runtime`; the public methods do not wrap it again | `panicking_initialize_waiter_leaves_the_worker_answering`, `panicking_first_identity_waiter_leaves_the_worker_answering` |
| `RecordStorage::read_committed` | The `nessa-committed-read` thread, via its reply oneshot after the worker answers | Shutdown is not `ReadWorkerPanicked`. The worker keeps answering | `panicking_committed_read_waiter_is_not_a_shutdown_failure` |
| `ShutdownCompletion::wait` (`SessionStorage::shutdown`) | The shutdown task's `Notify::notify_waiters` | A second shutdown waiter is still woken and both see the stored outcome | `panicking_shutdown_waiter_still_wakes_the_other_shutdown` |
| `CommittedChangeWatch::changed` | The save task's `Notify::notify_one`, after the dirty bit is set | The save result stays successful, the notice stays `Dirty`, and a later shutdown is `Closed`. `NotificationFailed` is not a state of this watch | `record_watch_callback_panic_cannot_replace_a_durable_save_result` |

The panicking wait itself loses that one wake; the SDK does not retry it.
Polling it again returns the retained result.

The two `RecordStorage::runtime` tests hold the exclusive lock only until the
caller waker is parked. `ready.send` is after `open_connection`, so their
deadline includes that cold open. The deadline is a hang ceiling for an open
that is still running, covering the connection's busy timeout and a blocking
pool shared with the rest of the crate's storage tests. A shorter ceiling
expired while `open_connection` had not sent `ready` (#548, #558). An expiry
names the wait that is still active: `held_exclusive_lock_past_the_deadline_is_still_cold_open` reports
open that has not published `ready`,
`stored_ready_without_the_caller_waker_is_not_cold_open` reports `ready`
stored without that caller waker, and
`stored_identity_ready_without_the_caller_waker_is_a_later_statement`
reports `ready` stored while the call is pending on a later statement. The
other `caller_wakes` tests open the runtime first and then time one locked
statement.

Not covered:

- `Agent::prepare`, `SessionManager::open` and `SessionSnapshot::load_saved`.
  They await caller-supplied provider and storage ports. Those ports' tasks,
  when the port is this SDK's record storage, are the rows above. A port the
  caller supplies owns its own tasks; this primitive is not inside the
  caller's adapter.
- Store calls after the runtime exists: `open`, `open_existing`,
  `record_identity`, `record_source` and `record_source_expected`. They go
  through event-stream `tracked_read`, which completes on a Tokio task. That
  task already catches a plain waker panic, so the SQLite worker is not the
  thread that wakes the caller. The cold open of the runtime is the
  `RecordStorage::runtime` row, not a second wrap on these methods.
  `caller_wakes` in `tests/infrastructure/session_storage/` shows the worker
  still answers. A payload whose drop panics is the same Tokio-task exposure
  as a `JoinHandle`, not a second owner of this rule.

## Live model and permission configuration

`Agent::set_model(target, actor)` retains the existing native provider session and
any active execution. `ModelChange::between` checks the exact prior/target model
identities and provider-owned stable execution-context proof. Normal preparation
still refuses an arbitrary identity mismatch; only an explicit model-change fact
can rebind stored history. Claude applies a confirmed choice to subsequent
responses; Codex applies it to the next native turn. The returned
`ModelChangeEffect` reports this boundary and the complete confirmed model
capabilities, permission preset and effort. A target model may change the latter
two settings; the SDK validates the acknowledgement together rather than retaining
an old effort under a new model name. Claude's output ceiling is a launch setting:
changing that ceiling requires another launch and is refused before a live model
request. Changing its model with the same ceiling is supported.

The scheduler serializes model/settings controls with admission. Live model and
changing approval-preset controls preserve the configuration of unstarted admitted
input, including queued, selected and preparing work. The common audited control wrapper asks its existing
work permit whether another owner remains Waiting or selected/preparing Active,
and returns Busy before native effect. It does not reinterpret waiting input against a new selection.
The control permit excludes its own identity. Existing lifecycle retirement
requires retired non-waiting owners to drain before reopening; waiting owners
survive automatic recovery and remain blockers until execution or retirement.
Failed-attachment settlement retains the scheduler lock until its Settling owners
retire, and failed-provider readiness refuses control admission. Public controls
therefore do not independently reach the guard with a Settling owner; the
exhaustive match conservatively retains that phase without a separate public
omission-coverage claim.
An Executing turn alone still permits supported native model control and keeps its
original execution and trace identity; later admission records the current
configured model. The trace store belongs to the Agent
lifetime, so replacing a model factory cannot orphan a span started earlier.
`Agent::set_approval_mode(mode, actor)` also permits an active owner turn. Neither
control answers, replaces or cancels existing permission requests. A checked grant
must continue to use Ask with native hooks disabled: relaxing it is refused before
dispatch, and a model acknowledgement that unexpectedly relaxes it fences the
uncertain generation. `agents::models` tests these active-turn relationships.

`SessionSnapshot::current_configuration` is the latest attributed settings owner,
including its exact before/after configuration and typed change kind. Historical
`model_change` separately identifies the original model operation for host-journal
recovery; a later permission choice does not rewrite that operation. Both facts
are saved before success is published. `SessionManager::saved_configuration` and
`saved_model_change` read committed evidence without opening a provider. A cold
`SessionManager::change_model` requires settled history and preserves its native
context; it is not a live-control shortcut. An unfinished save remains unresolved
until the storage owner reconciles it.

Factories with live settings controls must implement `with_model_configuration`
as a pure, validated clone retaining their host/lease/credential authority and
trace store. A changed setting is preflighted through that port before native
mutation. Restore needs no clone when the supplied factory already has the exact
saved configuration; otherwise unsupported normalization fails before opening.
`Agent::capabilities` returns an owned current snapshot, not a lifetime-long model
promise. Keep identity/capabilities from one control result together when applying
host projections.

`ProviderOperationFailure::stage()` distinguishes SDK-owned `BeforeEffect`
refusals from `EffectUncertain`; `Usable` alone does not prove no native mutation.
After backend dispatch, failures are always uncertain: an adapter cannot reuse
a `BeforeEffect` failure obtained from another SDK operation. Only this operation's
local preflight branches retain that proof. `ConfigurationControlled`
audit keeps the original requested model/preset separately from the native
acknowledgement and SDK persistence outcome. A verified native acknowledgement is
recorded before saving current configuration, so a subsequent storage failure does
not erase the already observed native settings. Before-effect refusals and uncertain
failures retain the same actor and attachment identity as their request.
Uncertain model replies,
contradictory acknowledgements and failed durable saves fence further work rather
than publishing the requested configuration. The owned control task settles after
a caller drops its wait, and concurrent close retains the existing generation
fence. Scripted `acp::contracts::model_switch` tests exercise both actual profiles
through the process boundary, including original/new trace provenance.


The configuration settlement order is designed around the existing work permit,
not another control lifetime. The accepted #922 recovery contract conservatively
fences its captured current attachment on any admitted-owner unwind, including preflight, before retiring
work. This matches the public `CleanupRequired` result without claiming native
application. Ordinary drop and caller loss do not imply unwind; the native poll
boundary still owns native stop/receipt ordering.
`ModelConfiguration` owns cloning and validating a configured factory, shared by
model preflight, native acknowledgement and restore. The retained configuration
record owns its relationship to the historical model record; semantic folding and
snapshot validation both ask that owner.

| State and event/order | Settlement and retained evidence | Regression |
| --- | --- | --- |
| Saved completed A submission, then B narrows context/output/image support, then exact Queue/Boundary/Steer retry | Saved request identity and receipt resolve before fresh CURRENT-configuration validation; no extra dispatch, save or admission audit. All nine before-probes rejected the exact receipt against B | `models::admission::saved_*_retry_after_*` (nine cases) |
| Same-ID changed request after B, or fresh invalid request under B | Saved identity mismatch remains SubmissionConflict; genuinely new input still uses current configured validation. Before-probes incorrectly validated conflicts against B while fresh-invalid refusals remained correct | same nine public retry cases; `scheduling_retry_rejects_changed_content_attribution_or_delivery_mode` includes a changed image under the same identity |
| Sole queued checked input selected and blocked in attached provider preparation; queue empty and active identity absent; changing model/preset requested | The common audited wrapper asks its own WorkPermit before polling a changing control: Waiting and selected/preparing Active other owners return Busy BeforeEffect; Executing owners remain allowed. Both before-probes applied the control and lost the checked turn | `selected_model_retains_admission_authority`; `selected_preset_retains_admission_authority` |
| Same selected preparation ordering with unchanged Ask preset | The wrapper uses captured before_configuration to exempt an unchanged preset; native control and checked input complete. Public before-positive passed | `selected_unchanged_preset_remains_supported` |
| Active A plus ordinary/boundary/PromptRequired input admitted under A; target B has smaller limits | Busy BeforeEffect, Requested/Refused audit, zero native model calls and no Applied/model fact; all admitted inputs complete under A | `queued_limits_keep_the_admitted_configuration_authority` |
| Same ordering, target B removes the admitted image modality | Same refusal and preserved receipts; no dispatch-time reinterpretation | `queued_images_keep_the_admitted_configuration_authority` |
| Same ordering, checked-grant input needs Ask and B would relax it | Same refusal and preserved checked grant; no queued tool-policy invalidation | `queued_checked_grants_keep_the_admitted_configuration_authority` |
| Owner A active plus queued checked-grant Ask input; direct preset would relax Ask | Shared work-permit prerequisite returns Busy BeforeEffect for a changing preset and preserves all receipts. Before correction, Full succeeded and queued checked input failed Unsupported; active-only control remains allowed | `queued_preset_keeps_the_admitted_configuration_authority` |
| Queued checked-grant Ask input and an unchanged Ask preset request | Existing same-value native control and audit behavior remains allowed; the pending-work prerequisite exempts an unchanged preset. Public before-probe passed | `queued_unchanged_preset_keeps_the_admitted_configuration_authority` |
| Only a native-executing active input, no queued or retained pending owner | Supported native model control remains allowed; original active/review identities persist | `a_live_model_change_keeps_the_active_turn_and_context_and_changes_later_admission`; active checked-turn tests |
| Native model ACK, then configured factory panics | Work-permit unwind fences admission; no Applied or committed model is published; explicit close owns physical cleanup | `post_ack_factory_panic_fences_admission_without_committing_model` |
| Factory panics during preflight, before native entry | Zero native dispatch and no Applied/committed model; `CleanupRequired` fences subsequent admission until explicit close and restoration | `preflight_factory_panic_fences_admission_until_cleanup` |
| Ordinary permit drop after completed settlement | No unwind and no fence; later model/settings controls remain usable | adjusted-ACK restore test |
| Old admitted permit unwinds after its captured generation is replaced | Existing `apply_control_state` generation check prevents fencing the replacement | `permit_unwind_fences_current_but_not_retired_attachment_or_ordinary_drop` |
| Caller drops wait after dispatch | Owned control continues through acknowledgement and persistence; caller loss does not interrupt settlement | `dropped_model_caller_keeps_owned_settlement_and_original_actor` |
| Native ACK adjusts mode or effort, factory retains the adjusted settings | Save exact acknowledged settings and install that configured factory | `adjusted_acknowledgements_install_and_restore_complete_configuration` |
| Native ACK adjusts mode or effort, factory ignores the adjustment | Refuse uncertain, fence admission, and keep prior committed model and settings | `ignored_acknowledged_settings_fence_without_committing_model` |
| Malformed retained configuration precedes a valid later semantic control | Canonical continuation restore rejects the prior fact before the later control can replace it; checkpoint and custom-store reads ask the same owner | `configuration_relationships_match_semantic_fold_and_checkpoint_restore`; `custom_storage_cannot_restore_model_fact_without_its_current_configuration` |
| Checkpoint/custom store has an approval change that changes effort, or effort change that changes mode | Reject before opening; semantic application uses the same relationship rule | `configuration_relationships_match_semantic_fold_and_checkpoint_restore` |
| Latest configuration says Model but its before/after/actor disagrees with retained model, or either fact is missing | Reject before opening and preserve existing committed evidence | same relationship matrix |
| A valid later permission or effort choice follows a model change | Preserve historical model actor/settings separately; restore latest settings | same relationship matrix and adjusted-ACK restore test |


The #922 R2 structural correction separates compatible saved configuration from
live harness-host ownership. A configuration fingerprint describes launch inputs;
it cannot prove that equal workspace paths name the same host. The accepted
contract compares the captured host owner separately: local factories agree with
local factories, and hosted factories agree only when they borrow the same host
`Arc`. The provider port supplies an opaque borrowed `HarnessHostIdentity`; it
exposes no start-capable handle and is never persisted. The existing
`ModelConfiguration::configured_provider` owner also checks that normalization
retains this identity, so a settings clone cannot bypass the model-change check.
Every provider must explicitly report its live host identity: local factories
assert local execution, hosted factories borrow their captured host owner, and
wrappers delegate to the wrapped factory. There is no default assertion. This
supersedes the earlier default-local design: a forgotten wrapper must fail to
compile rather than silently discard the wrapped authority. An SSH lease retains
the one host Arc created at grant and reuses it for every target factory.
On cold restore composition supplies the host, and normalization must preserve
that supplied owner before opening. Saved fingerprints establish configuration
compatibility, not historical host identity across process restarts.

| State and event/order | Result and retained authority | Regression |
| --- | --- | --- |
| Claude/Codex factories use different host Arcs with equal workspace/configuration | Refuse model change before native dispatch; keep original factory and history | `model_choices_require_the_same_captured_host_owner` |
| Factories share one host Arc, or both are local | Permit compatible model choice; hosted native change and close/reattach remain on the captured host | same owner matrix; hosted model-switch journey |
| Local/hosted crossing in either direction with matching configuration | Refuse before native dispatch | same owner matrix |
| Preflight normalization returns a different host with matching fingerprint | Refuse before native dispatch | `normalization_cannot_replace_the_captured_host_owner` |
| Adjusted native model ACK requires a clone that replaces its host | Return CleanupRequired and fence; do not install clone or commit model/Applied; original owner retains cleanup | same normalization matrix |
| Approval/effort normalization replaces the host | Refuse in existing preflight before native settings dispatch; same-host settings remain usable | same normalization matrix |
| Cold restore settings require a clone that replaces the supplied host | Refuse before provider open; same-host normalization preserves supplied owner and restores once | same normalization matrix |

| Provider implementation omits its host assertion | Compilation fails with E0046; adding a default invalidates the compile-fail example | `AgentProvider::harness_host_identity` compile-fail example |
| LeaseFence wraps a hosted factory, including configured clones | Delegate live host identity and retain the same lease/fence owner; foreign host with equal attributes is refused | `hosted_lease_fence_and_normalization_delegate_the_captured_host_owner` |
| SSH lease adapts a model target after grant | Reuse the original grant-created host Arc; target normalization and reopen retain that owner; grant/end once | `model_targets_and_normalization_retain_the_granted_host` |

### Native handoff and admitted configuration

An entered backend future is not a native handoff. Existing Active work must
retain its admitted configuration through image reads and worker preparation.
The existing permit owns the fallible transition to Executing; an adapter must
invoke its handoff only after readiness, profile and frame checks, immediately
before its serialized native owner begins the effect. Stop or retirement makes a
late handoff refuse. Executing work continues to permit supported live controls.

| Ordering | Required ownership and result | Evidence |
| --- | --- | --- |
| Direct or queued A waits for image bytes; model B requested | Active A returns Busy BeforeEffect for B, no native model command; releasing bytes dispatches A once with original receipt and identity | Real ACP UserImageSource barrier; NextTurn and SubsequentResponses, equal and unequal ceilings; actual before eight failures; fixed ten pass (including Stop). Eager-transition omission reproduces eight failures/two Stop positives; omitted worker transition reproduces eight native-live failures/two Stop positives; exact restoration ten pass |
| A has crossed serialized native handoff; model B requested | Existing supported live control remains allowed; subsequent queued input uses B | Same public ACP journeys: one native model change after A starts, fresh B prompt, both traces and original saved identities; ten pass |
| Ready policy drift or profile/frame refusal precedes handoff | Existing worker refuses without calling the permit transition or beginning a native turn | Existing ready-policy refusal leaves count zero; worker frame refusal count zero and fallible Closed handoff forbids native begin; actual regressions pass. Profile check remains immediately before frame construction, with actual ACP unequal-ceiling validation evidence |
| Stop retires A while image preparation is pending | A late adapter handoff refuses and sends no native prompt | Direct/queued image-read Stop tests pass with zero native prompts/model commands; dropped-work, replaced-attachment and stopped-generation handoff refusal is tested at the existing permit owner |

The model-control test backend's held turn owns its simulated native invocation at
its `started` notification, before its release wait. Its required port migration
must invoke handoff there, then forward the same idempotent callback when its
inner backend settles. Merely forwarding at the end incorrectly labels an
already native fixture turn as preparing; the existing 52-case model suite caught
that migration error (39 passed, 13 Busy failures).

### daf57 current-main integration

| Relationship | Existing owner retained by integration | Qualification |
| --- | --- | --- |
| Model/preset settings and committed snapshot after a failed save | Main's Continuation applies ConfigurationChanged/ModelChanged units through evidence.record; their existing forward and undo arms retain provider, model-change record and current configuration together. SDK setters do not mutate a second expected snapshot | Assembled571 full SDK runtime: 2,396 passed, 23 ignored; storage/configuration/model suites included |
| Cold model selection before attachment | Existing complete saved-history validation, exact prior identity and no pending work; storage derives the replacement snapshot from the single ModelChanged unit | Assembled571 cold model-change and persistence suites included in 2,396 SDK passes |
| Required native handoff and incoming resumption/records | WorkPermit and serialized ACP callback remain unchanged; incoming record continuation, resumption readiness, exact Stop identity and fresh unit save contract are preserved | Focused handoff rules and adapter bodies are checkpoint-identical; assembled571 SDK 2,396/server 3,291 runtime tests pass. Window/gate19 stress remains pending |

### Persistent settings on current-main recovery

| Ordering / relationship | Single owner and intentional contract | Qualification |
| --- | --- | --- |
| A verified model/preset choice, then explicit or automatic reattachment | The normalized current_provider and durable current_configuration retain the choice. Opening that factory configures native settings before publication. This intentionally supersedes main's former caller-attachment reset to the original factory preset; no live_approval_mode or carry cache is reintroduced | Preserve automatic-recovery, admission, Stop and failure negatives; migrate only reset assertions. Assembled571 SDK 2,396/server 3,291 runtime tests pass; browser/stress pending |
| Admission saved while an attachment closes or recovers | Scheduler holds configuration controls and each unstarted WorkPermit blocks changing controls. Capture the preset before the admission save from the current immutable factory; WorkPermit retains its exact work/provider generations for Stop and late handoff. Detached recovery does not reset the factory | Incoming approval/admission and public handoff regressions; assembled571 SDK runtime passes; browser/stress pending |
| Exact saved retry after a later selection | Recover the saved receipt before fresh required-mode or capability checks; keep its full ExecutionRequest and initiation, including reservation and app contexts | Existing saved queue/boundary/native retries and incoming carried-context/reopen regressions; assembled571 SDK runtime passes; browser/stress pending |
| Malformed custom snapshot followed by a valid configuration unit | Main owns unit validation through `SessionSaveUnit::fold(prior, units)`, which validates the prior before applying a later unit. Replace the removed candidate-validation test API without weakening malformed-prior or zero-provider-open assertions | First assembled compile failed E0599 at models.rs1309; the incoming save-cost snapshot also requires explicit absent model/configuration fields (E0063). Current public fold regression and unchanged save-cost fixture with no model control pass in the assembled suite |

### Unsupported preset in a model acknowledgement

| Relationship | Existing owner and expected result | Evidence |
| --- | --- | --- |
| Target factory accepts Ask only; preflight requests Ask; native model ACK reports Auto | The same target factory validates the complete acknowledged settings. Its Unsupported is post-effect uncertainty: CleanupRequired, one native call, no durable model/configuration commit, new admission fenced until cleanup. Do not relax profile capabilities to accommodate an invalid scripted ACK | Unchanged production passes the new public assertion. Isolated sole post-ACK target-normalization omission wrongly returns success and fails that assertion; source-preserving fresh guarded rebuild passes all 53 model tests. Exact seven-package all-target Clippy passes. This covers an existing guard, not a new production fix |

### Normalized native-hook agreement (#975 R4)

`ModelConfiguration::configured_provider` must preserve the supplied factory's
`native_hooks_allowed` fact alongside identity, capabilities, settings, context
and live host owner. Model, approval, effort and saved-setting restoration share
this owner. A settings clone cannot grant hooks that its supplied factory denies.

| Ordering | Required outcome | Evidence |
| --- | --- | --- |
| Model preflight or approval/effort preflight returns a clone with different native-hook policy | BeforeEffect refusal, zero native calls, unchanged durable configuration; admitted work remains usable | Public Agent model/approval/effort preflight cases: old production accepts hook-changing clones; fixed refuses; private sole-check omission reproduces |
| Native ACK adjustment returns a clone with different native-hook policy | EffectUncertain / CleanupRequired, one native call, no new durable configuration or Applied audit, original actor retained; admission fenced until once-only cleanup | Public Agent adjusted-ACK case: old production accepts; fixed fences; private sole-check omission reproduces |
| Saved settings need a clone with different native-hook policy | Refuse before opening; exact clone still restores original settings and hook policy | Public Agent saved-setting case: old production opens wrong policy; fixed refuses before open; exact clone restores |
| Exact clones preserve denied or allowed native hooks through explicit or automatic reattachment | Both opens retain the supplied policy; checked-turn admission keeps its existing contract | Public Agent exact-clone matrix covers false/true policy × caller-requested/automatic recovery, with two policy-preserving opens. Existing active checked-turn control regressions remain required |

The six public hook-normalization tests produce five intended old-production
failures and one positive matrix. Fixed and private exact-restored runs pass all
six; omitting only native-hook equality in an isolated archive/target restores
the five failures while the exact-clone positive passes. Earlier incomplete
reattachment fixture setup runs remain excluded from that evidence.

### Effort acknowledgement and terminal settlement (#975 R5)

The existing `change_effort_level` owner derives the native outcome before local
configuration persistence and retention, then attempts one bounded terminal audit
with its original `EffortLevelChange` while holding the scheduler and permit.
`Applied` means the native level was verified, not that configuration storage
succeeded. A later local failure is still returned and fences this generation.

| Ordering | Required outcome | Public evidence |
| --- | --- | --- |
| Native ACK, configuration save succeeds and settings retained | Requested then Applied, exact durable settings, admission usable | Existing effort positives plus forwarding-store success; new public positive passes |
| Native ACK, configuration save refuses | Requested then Applied; primary Storage error / CleanupRequired; no committed configuration at failed return, admission fenced, cleanup once | Forwarding real storage regression; old production fails, fixed and private exact restoration pass |
| Native ACK, retention clone refuses or panics | Requested then Applied; original normalization error or typed panic failure / CleanupRequired; saved native settings retained, admission fenced, cleanup once | Public returned-refusal and panic cases; old production fails, fixed and private restoration pass |
| Native ACK, local settlement error and terminal audit error | One terminal attempt, primary local error first and audit error second; CleanupRequired | Storage+audit and retention+audit cases preserve primary-first; fixed and private restoration pass |
| Native refusal or unverified failure, or caller drops wait | Existing Refused/Failed semantics and owned settlement remain; backend/audit panic containment does not invent an ACK | Existing effort family plus native unverified/panic positives; scoped qualification below |

Runtime teardown may prevent any future polling; no terminal audit delivery is
claimed after the owning runtime has been destroyed. Model/approval terminal
records remain owned by their existing outer `audit_configuration_control`.

The eight new public settlement cases produce five intended old-production
failures and three positives. Fixed and private exact-restored runs pass all
eight. Omitting only the common terminal audit for known ACK plus local failure
in an isolated archive/target reproduces the same five failures. Native effect
and local settlement results remain separate; terminal audit is attempted once.

### Shared model and preset supervision (#975 R6)

`audit_configuration_control` owns supervision of its entire operation and
success acknowledgement extraction, and one bounded terminal audit attempt.
A panic becomes SubmissionUnresolved / CleanupRequired under the captured
permit before faulted future cleanup; panic payload destruction cannot bypass
settlement. It does not prove native effect or turn a panic into BeforeEffect.
The scheduler and permit remain held through terminal audit.

| Ordering | Required outcome | Public evidence |
| --- | --- | --- |
| Model preflight or adjusted-ACK factory panics | Requested then Failed, original actor, no fabricated verified ACK/Applied; native count0 or1, prior model/configuration retained, new admission fenced, cleanup once | Existing post-ACK case fails old production on required Failed; public model/preflight cases included in affected family |
| Approval native ACK and settings save succeed, retention factory panics | Requested, Acknowledged, Failed; preserve saved native Auto and original actor; uncertain failure and cleanup once | Public before matrix: two intended failures and two exact-clone positives; fixed and private exact restoration pass |
| Either panic plus terminal audit rejection | Exactly one Failed attempt, primary SubmissionUnresolved before audit error; CleanupRequired retained | Model and approval combined rejection cases pass with primary uncertainty first; attempts are not delivery claims |
| Exact settings clones or caller drops wait | Existing single Requested/Acknowledged/Applied settlement and attributed owned task remain | Exact-clone positives and existing caller-drop model regressions |
| Factory panic payload has a hostile destructor | Contain and forget payload; destructor never runs, original Failed settlement still attempted | Public hostile-payload case passes with destructor count0 |

The existing bounded audit observer contains audit sink construction/poll/drop
faults. No terminal delivery is promised after owning-runtime destruction.
Effort retains its separate native-outcome/local-result settlement owner.

The guarded public matrix passes all seven cases. Private isolated removal of
only operation-poll supervision reproduces four model/preset failures while two
exact-clone positives pass; the hostile payload is deliberately excluded from
that unguarded run. Exact fresh-mtime restoration recompiles and passes all
seven, including the hostile payload. No checkout source variant is used.

The original preflight-panic regression required only Requested. Its assertion
now requires Requested then Failed on both records with its original custom
actor, target and no acknowledgement. Native dispatch remains0 and physical
cleanup/reopen assertions remain unchanged; the first stale-assertion failure is
retained separately.

### Effort owned computation supervision (#975 R8 follow-up)

After successful Requested, the existing effort settlement owner must retain
its original change/actor, scheduler and permit through all configuration
callbacks, native future construction/poll, local settlement and future cleanup.
Native outcome stays separate from local errors: an observed native Ok is
Applied even when a later callback unwinds; an earlier unwind is Failed, never
an invented ACK. Every such exit reaches the same bounded terminal attempt.
Ordinary repository before evidence confirms the gap: the getter pair gives
one intended missing-terminal failure and one exact-getter positive; the wider
initial matrix gives four missing-terminal failures, nine positives and two
incorrect storage-error expectations. The latter are fixture mistakes: the
existing storage owner already contains future cleanup and returns typed
Storage, which must stay primary. All original logs remain distinct.
The serialized effort contract reuses the captured before preset after ACK;
it does not call an equivalent getter again merely to construct the new value.
A required later normalization getter can still fail after configuration save;
that case retains the exact saved low level and original actor, plus Applied
native evidence, while returning uncertainty and fencing admission.

| Ordering | Required evidence and failure | Repository evidence |
| --- | --- | --- |
| Getter after Requested, before native dispatch, unwinds | Requested then Failed; no native call or durable change; uncertain typed failure/CleanupRequired, admission fenced and cleanup once | Ordinary public Agent before failure; fixed matrix passes |
| Getter after verified native effort ACK unwinds | Requested then Applied with original actor and levels; native count1, primary uncertain typed failure/CleanupRequired; exact storage state retained, admission fenced and cleanup once | Ordinary public Agent before failure; fixed matrix passes |
| Native construction/poll, retention or owned future drop unwinds | Same computation supervisor retains known native outcome, fences before cleanup and forgets panic payload; no per-getter or per-retain audit alternative | Ordinary public Agent constructor, retention and native-cleanup matrix; existing caller-drop/audit fixtures |
| Exact getters, unsent refusal, storage error or combined terminal audit error | Preserve normal success/Refused, primary-first errors and one bounded terminal attempt | Existing public effort positives and affected settlement matrix |

A native future that returns an unsent refusal but unwinds during cleanup has
not delivered that verified reply to the effort owner. Its terminal is Failed,
with no invented ACK, no native effect/save and cleanup once. A storage future
cleanup fault is already contained by the storage owner: after known native ACK
it retains Applied and the exact typed Storage primary (before any audit error).
Neither case adds another cleanup or error-policy owner.

The corrected ordinary repository baseline preserves the old retention-panic
error type and yields five missing-terminal failures and eleven positives,
including both exact typed Storage cleanup cases. Private isolated removal of
only operation-poll supervision yields six failures and ten positives; its
additional retention-panic failure confirms that the narrow callback catch is
now replaced by the common computation supervisor. Exact fresh-mtime source
restoration recompiles and passes all sixteen. Protected drop and the single
terminal attempt remain present in the omission. The original mixed baseline
and the first fixed run's two stale storage assertions remain separate records.
This evidence covers executable task completion, not audit delivery after
runtime destruction.

## Reports from supervised close and suspend

`Agent::close_report(action)` and `Agent::suspend_report(action)` return an
`AgentCloseReport` from the same supervised transaction used by `close` and
`suspend`. They add no cleanup owner or retry. Final cancellation stays final;
suspend retains its owed-work meaning. Dropping the caller stops its wait, while
the transaction continues on the owning Tokio runtime. The outer error means
that transaction did not return a report.

Read `cleanup()` for the full lifecycle `CleanupReport`, and `scheduling()` for
pending-work cancellation persistence and audit. Confirmed resources can coexist
with failed audit or completion. `ResourceCleanup::ReleasePending` confirms
process cleanup while retaining an executable-use release owner; a successful
`physical_outcome()` alone does not acknowledge that owner. The report's
`operation_failure` retains its historical meaning and is not automatically a
failure of an otherwise successful cleanup.

`into_result()` gives the ordinary close result, retaining scheduling storage
failure together with its cleanup result and other independent failures. It
does not mutate Agent ownership. An encompassing environment's real cleanup
receipt confirms that environment's physical release. Whole SDK release still
requires its actual attachment owner's acknowledgement, including any
executable-use guard; a derived report cannot acknowledge that obligation. A
report concerns its transaction, not an aggregate claim about an Agent that a
later invocation has already attached again.

The `close_reports` public conformance matrix checks both modes, physical facts,
forced status and independent fields. Scheduling tests
`report_modes_keep_final_and_suspend_pending_work_semantics` and
`report_retains_failed_pending_cancellation_separately_from_cleanup` check saved
work and failed evidence. The gateway's
`remote_ack_cannot_release_the_sdk_executable_use_owner` checks the separate
remote and SDK release acknowledgements.

## Historical decline acknowledgement

`decline_turn` supervises its receipt through caller loss. It first saves a
`DeclineDecision` preparation containing the original attributed terminal edge.
Only an acknowledged preparation can apply NotResumed and publish its audit.
Once the terminal edge is applied locally, audit is attempted independently even
if saving that edge fails. The first preparation remains authoritative when
another caller retries. `retry_declined_turns()` joins pending receipts without
choosing another actor or resuming work, including after restoration with an
empty owed list. A prepared decline excludes a competing resume.

`InvocationRecord::decline` retains preparation and audit acknowledgement.
Application/snapshot validation correlates the legal owed stop, original edge,
caller and acknowledgement. Audit acknowledgement cannot precede the terminal
edge or regress. A durable audit adapter must make repeated Declined input for
the same session/execution idempotent: preserve original metadata and reject a
conflicting semantic payload. An acknowledgement save failure remains a storage
failure; a later flush can publish that already-observed receipt without another
audit delivery. No audit delivery is inferred from older untracked NotResumed
history, and no provider/tool work is replayed.

`AgentCloseReport::after_retry` joins an actual later observation from the same
cleanup owner. Its resource observation supersedes earlier uncertainty while
independent audit, scheduling and completion failures remain observable. The
value itself releases no Agent resources. In particular, remote physical End
must be reconciled through that SDK owner; Unconfirmed can still contain an
unattempted executable-use guard. Public report projections are not release
acknowledgements. SDK close/suspend also joins already-prepared decline receipts;
it does not automatically choose to decline otherwise owed work.
