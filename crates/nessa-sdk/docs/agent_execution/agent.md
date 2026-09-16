# Agent entry point

Construct one `Agent` for a conversation. Give it an execution provider and a
`SessionManager`, register hooks, then submit messages. The public API is the Rust SDK;
the TypeScript gateway client does not yet expose agent chat RPCs.

```rust,ignore
let provider = Arc::new(ClaudeAcpProvider::new(config, &model, limits, audit)?);
let storage = Arc::new(LocalFileStorage::new("./sessions")?);
let manager = SessionManager::open(None, storage).await?; // Fresh local UUID v4.
let session_id = manager.id().clone(); // Retain this key to reopen the conversation.
let agent = Agent::new(provider, manager).await?;

agent.add_hook(BeforeInvocation, |context: &InvocationContext<'_>| {
    tracing::debug!(execution = context.request.execution_id.as_str(), "Invoking agent");
    Ok(())
});

let receipt = agent.enqueue(request, verified_actor).await?;
let result = receipt.wait().await?;
```

`request` is an `ExecutionRequest`: a fresh execution ID, validated `PromptText`,
and explicit input/output token budgets. `verified_actor` is an `ActionContext`
constructed by the host after authorization. The SDK does not guess token counts
or treat a caller-supplied principal string as authenticated identity. See the
[complete queued interaction](../../src/application/agent_execution/agents/agent.rs)
and the [executable provider composition example](../../examples/claude_acp.rs) for composition.

Omit the local key with `SessionManager::open(None, storage)` to create a fresh
UUID v4 session. For a named or existing session, pass `Some(SessionId::new("test-session")?)`.
Save `manager.id().clone()` to reopen a generated session; `None` always creates a new key.
This session UUID identifies the conversation. Each new logical submission also
needs a separate `ExecutionId`; retries preserve that submission ID. The SDK does
not generate execution IDs automatically.

## Ownership and call flow

```text
host / future chat RPC
    -> SessionManager::open(optional local SessionId, storage)
        -> SessionStorage::open -> SessionStorageLease [exclusive writer lease]
    -> Agent::new(provider, session_manager)
        -> load saved snapshot and verify provider identity
        -> AgentProvider::open(saved provider context or None)
        -> save attached context identity
    -> Agent::add_hook(event, callback)
    -> Agent::enqueue(request, actor)
        -> capability validation + saved queue admission
        -> wait for the single invocation slot
        -> save submitted input and caller attribution
        -> before hooks
        -> provider prepares/restores its live context
        -> provider executes while Agent drains observations
        -> stream text; save at tool/review/terminal boundaries
        -> save settlement -> after hooks -> result
```

Arrows mean calls or delivery order. `Agent` owns orchestration and all public
controls: invocation, queued admission, steering, withdrawal, observation,
permission responses, and close. Clones share the provider context, manager, hook registrations, and
invocation gate. Overlapping immediate `invoke` calls return `Busy`; sequential
calls reuse context. Queued admission can succeed while an invocation is active.
Use `enqueue` for FIFO follow-ups, `enqueue_steering` for priority at the next
invocation boundary, and `steer` for provider-supported live injection. See
[scheduling](scheduling.md) for admission, cancellation, and delivery guarantees.

`providers::AgentProvider` is the application port for a complete execution
runtime. Its `ProviderIdentity` identifies the provider, exact model, and context
configuration required for restoration. ACP implements this port through
`ClaudeAcpProvider`. A future direct-model implementation can implement the same
port while owning its model/tool loop. No direct-model adapter is shipped yet.
`ProviderSession`, `ProviderSessionBackend`, and `ExecutionEventStream` are adapter
contracts for adapter authors. Adapters construct the control/event pair, while
ProviderSession runtime controls are crate-private. Applications invoke and control
Agent; adapter transport contracts are tested inside the crate.

Adapters receive a typed `SessionCloseRequest`: explicit close retains the verified
caller, while execution failure, deadline, lost event consumer, and dropped session
handles remain distinct causes. An application failure does not imply its handles
were dropped. The adapter carries the cause into closure and permission audit;
cleanup confirmation is a separate result.

## Model capabilities and provider operations

`agent.capabilities()` returns the immutable effective model capabilities selected
at creation: modalities, model features, and configured token limits. Every input
is validated against them before provider dispatch. Each new message also has a
4 MiB UTF-8 byte limit (`ExecutionRequest::MAX_MESSAGE_BYTES`), checked before
admission clones or saves it. The caller’s token estimate cannot bypass this
limit. It is a per-message retention guard, not tokenization or a total-history
budget; provider framing and model limits can impose lower bounds. Restored
requests obey the same byte limit.

`agent.operation_capabilities()` returns a separate `OperationCapabilities`
snapshot with `native_steering` and `session_resume`. ACP fills this from the
successful connection negotiation and rechecks it whenever the provider context
is restored. During reconnection both fields are false until validation succeeds;
closing retains the last negotiation. Custom provider backends advertise neither
operation unless they explicitly implement this accessor.

Support is a UI hint, not an admission permit: a supported operation can still
fail because the target finished, the connection failed, or saved history is
unavailable. Read the snapshot again after reconnection. SDK-owned queueing,
next-invocation steering, and invocation hooks remain available independently of
native provider support. No provider model/tool-step hooks are implied.

## Session manager and storage

`SessionId` is the local conversation key. `ExecutionSessionId` is the provider's
opaque context ID. They have different lifetimes and must not be substituted.
The manager stores the association and passes the provider ID back when another
Agent is constructed with the same local key. A different provider identity is
rejected before opening a context. Restoration must return the same provider ID;
missing history or unsupported resume fails explicitly. Construct provider identity
with `ProviderIdentity::new(name, model_id, context)`: provider/model fields each
have a 256 UTF-8 byte limit, and the credential-free context has a 4,096-byte
limit. Construction preserves exact text and discards spare allocation capacity;
malformed stored identities fail before provider opening or rewriting.

`SessionSnapshot` contains submitted requests, their caller attribution, streamed
`ExecutionEvent` observations, original submission mode, scheduling transitions,
and recorded outcomes. `InvocationRecord::provider_report` separately retains
the provider result (if known), settlement source, observation failure,
provider session state, physical cleanup, and audit acknowledgement. A report
claiming local cancellation requires `local_cancellation`: the first stop captured
by that invocation's owner before the report was applied. It retains the cause and
verified closer, and must agree with any final scheduling cancellation. A provider's
own cancelled outcome remains a provider result. Missing or contradictory local-stop
evidence is rejected in live execution and restoration. The final
`result` is the local SDK receipt and may fail after the provider completed.
Restoration preserves these facts without inferring resource ownership from error
messages or nested diagnostic wrappers. Snapshots are application evidence.
`InvocationHistory` validates delivery intent, scheduling, terminal ordering, and
local settlement for both live recording and restoration. The domain execution aggregate continues to own live tool/permission
invariants. Provider-private history and model/tool internals remain with the
provider; stored prompts are never replayed automatically to reconstruct them.
A snapshot is not portable history that can be moved between arbitrary providers.
Restoration applies live tool counts, seen-review counts, and retained-payload
limits before opening a provider, including for custom storage. A later review
cancellation proves that review remained pending until that event. Successful
answers are absent from snapshots, so other reviews may have been answered
sequentially. Validation checks the least retention compatible with the saved
evidence; it cannot reconstruct missing answer timing or prove actual concurrency.
Borrowed payload checks run before validation copies observations. The manager
then compacts transferred snapshot allocations before opening a provider; this
requires one temporary full-history copy, without imposing a total-history cap.

- `InMemoryStorage` holds session snapshots for the lifetime of that storage
  instance. It can be injected into an actual application without model calls
  being simulated. Reconstructing an Agent with that instance retains its data.
- `LocalFileStorage` appends changes to a private JSONL file, synchronizes writes,
  and holds an OS writer lease. Separate agents/processes cannot write the same local
  session simultaneously. Create independent session IDs for separate chats.
- `SessionManager::snapshot()` returns the last acknowledged snapshot, or `None`
  before initialization. An unsuccessful save never appears there as committed.
- A failed input save prevents provider dispatch. Observation/save failures remain
  visible, with the execution result retained; necessary provider cleanup still
  runs. Queue/steering retries use the same original request and recover known
  delivery; immediate `invoke` has no receipt recovery. See [retries](scheduling.md#idempotent-submission-retries).
- A missing saved result has to be read with its scheduling evidence: input may
  still be queued, withdrawn, or injected into another execution. Otherwise it
  means settlement was not saved, not success or confirmed cancellation.
  Dropping an invocation future does not guarantee that provider work stopped;
  await `Agent::close(actor)` for verified cleanup. Such inputs are never replayed.

The manager acquires storage access during `SessionManager::open` and retains only
its required `SessionStorageLease`. The shared backend can open other sessions.
The lease prevents competing writers while the manager owns the conversation;
closing the provider does not release it. An attachment guard also retains the lease
until provider cleanup is confirmed. Dropping an unattached manager releases its
lease immediately; dropping an attached Agent starts supervised cleanup and retains
exclusion through uncertain results. Supervised admission saves and outstanding
file operations retain the same lease until they finish, even after their caller drops.

`Agent::new` returns `AgentInitializationError` on construction failure. Inspect
`cause()` for the typed failure and `needs_cleanup()` for retained attachment
ownership; `retry_cleanup().await` retries cleanup without opening a replacement
context or sending input. The original failure remains available after recovery.
Initialization is supervised once polled, so abandoning its future cannot release
the lease while provider opening, saving, or cleanup is still in progress. Failed
provider opening must supply actual owned resources through
`ProviderOpenError::with_cleanup` when termination is uncertain, even before a
provider session identity exists. Its fields are private; `no_resources(cause)`
rejects uncertain cleanup, including nested causes and bounded summaries, returning
the typed cause for the adapter to pair with its cleanup handle. Confirmed cleanup
and failures before attachment can use the validated resource-free constructor.
If an adapter panics without returning a cleanup handle, initialization returns
`CleanupUncertain`. No retry can prove termination in that case: the protective
lease remains held for the process lifetime, even after the error is dropped.
Adapters must transfer owned cleanup to support recovery.

Dropping a recovery error with an available cleanup handle delegates cleanup to
a task with bounded backoff. Adapter panics while constructing, polling, or
dropping a cleanup future leave cleanup unconfirmed and retain the same target
for retry.
Keep the Tokio runtime alive until cleanup finishes; shutting down that runtime
cannot establish that an external process stopped. If it cancels the last cleanup
task, the SDK keeps the storage lease fenced for the process lifetime. Await explicit cleanup before
host exit. Audit-only failure remains reported separately from confirmed resource
cleanup.

Streaming text and thoughts reach subscribers immediately and stay in memory
until the next save. Tool/review events, terminal observations, and invocation
settlement save the accumulated observations. There is no timer or per-chunk
write. If the process fails mid-stream, text since the last save may be lost;
recovery never reruns the provider to recover it. Exact chunks and their order are
preserved when saved. Admissions and consequential lifecycle changes still save
before successful acknowledgement. Mandatory permission audit remains separate.

The file adapter appends only changed records and new event/scheduling tails to
`s-<encoded-id>.jsonl`, paired with the stable `s-<encoded-id>.lock` lease file.
`encoded-id` is lowercase unpadded base32hex of the exact session ID bytes, so
`Chat` and `chat` remain independent even on case-insensitive filesystems. The
prefix avoids reserved device names; maximum-length IDs use at most 213 ASCII
bytes per filename. Each complete line represents one saved update. Restoration
rebuilds the snapshot without invoking the provider. An interrupted final line is
removed under the writer lease before another append; malformed complete records
are reported as corruption. Retries reconcile the last write before appending.

The application storage port still accepts a complete snapshot. Snapshot copying
and validation therefore still depend on history size, while file writes no longer
repeat unchanged history. Before allocating each complete record, the decoder
checks field, collection, and diagnostic limits with a bounded streaming pass.
Each changed invocation has a 160 MiB allowance for decoded text and structural
slots; the exact live retention limits still apply after mapping. Escaped text
counts by decoded bytes, and string tokens are bounded before parser allocation.
The allowance is not a total-history or process-memory cap. Loading reconstructs
the whole conversation and validates each saved checkpoint;
its work grows with both history size and checkpoint count. Retained history stays in
memory; the pending-input count limit does not bound total session history.
These adapters serve one local conversation, not the proposed gateway event stream. Permission answers and cancellations use the
required independent audit sink. It retains answer selection, response-write
observations, cancellation causes, and once-only live session closure (including idle closure). Snapshots do not replace that audit or
claim to capture every permission decision and its authorization evidence.

Run the ignored persistence workload when measuring save and restoration changes:

```sh
cargo test -p nessa-sdk --test infrastructure persistence_scale_with_repeated_checkpoints -- --ignored --nocapture
cargo test -p nessa-sdk --test infrastructure concurrent_session_persistence_and_scheduler_delay -- --ignored --nocapture
```

The first workload builds deterministic histories of 100, 1,000, and 10,000
invocations with ten checkpoints each, a 1,000-invocation history with 100 small
checkpoints, and one 10,000-chunk streamed turn with 100 checkpoints. It reports
cumulative save time, one full restoration time, and journal bytes. The second
workload runs eight 100-invocation sessions concurrently and reports total wall
time plus median and maximum task-start scheduling delay.

The harness does not infer CPU time or peak memory from wall time. On macOS, wrap
either command in `/usr/bin/time -lp`; on systems with GNU time, use
`/usr/bin/time -v`. Those tools record process CPU and maximum resident memory for
the complete test command. The harness has no timing, CPU, or memory assertion
because results depend on the host and build profile. Record the command, host,
profile, and tool output with any budget or result derived from it.

## UI and tests

`invoke` drains provider output even without a subscriber. Subscribe before
invoking when the UI needs live text or permission reviews. `AgentEvents` is a
bounded live projection; lag returns `Backpressure`, and the UI can reload the
manager's acknowledged snapshot; unfinished live text may not be there yet. Dropping a subscriber does not discard audit
records or stop Agent from saving output. Each text/thought observation has a
4 MiB UTF-8 payload limit (`ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES`).
`MessageChunk::text` and `MessageChunk::thought` construct immutable values and
discard spare capacity; `kind()`, `as_str()`, and `payload_bytes()` expose inspection.
Agent validates borrowed events before copying or saving them. Text/thought chunks
use that limit; tool observations and review payloads use the controller's existing
32 MiB budgets, including retained identities, offered choices, raw input string
capacity, and collection capacity. Cancellation input receives the same review
preflight. Aggregate history validation still checks counts, combined tool state,
and provable pending-review lifetimes; per-event checks do not replace those rules.
Oversize provider output triggers cleanup and a visible failure without publishing
or persisting that event. A separate per-invocation retention ceiling admits up to 128 MiB of observations
and 262,144 events. It counts every event kind, each event slot and execution ID,
owned payload capacity, and conservatively counts shared cancellation payloads;
spare event-vector slots are separately bounded by the event-count ceiling.
The borrowed check happens before copying or appending incoming output. Private
incremental accounting avoids rescanning old output for this check; restored and
custom-storage snapshots use the same limit before admission. Sparse replacement
and resolved reviews still consume history because their earlier observations remain saved.

Crossing this generous ceiling rejects only the next event, preserves the accepted
prefix, and reports `OutputRetentionLimit` with the actual provider settlement in
`ExecutionObservation`. Cleanup and mandatory audit still run; saved output does
not claim to contain the rejected event or subsequent cleanup observations. The
independent audit sink retains cleanup evidence. Confirmed cleanup releases
physical resources. Reuse also requires successful audit delivery; an audit failure remains visible across repeated close attempts.
An explicit Close racing with cleanup retains its admission barrier. Uncertain
cleanup still requires explicit recovery. This is not a claim that the provider returned
`OutputLimit`.

The 32 MiB ACP queue budget measures a different ownership stage and releases on
dequeue. The retained ceiling remains charged after draining. It applies separately
to each invocation, not to the lifetime conversation. Observed/committed/storage
copies, transient parsing, subscribers, audit consumers, allocator overhead, and
full-history snapshot copying/validation remain separate costs; this is not an RSS ceiling.
A failed provider event reader or contradictory terminal outcome triggers
provider cleanup. A terminal observation followed by a failed execution result also
requires protocol cleanup: the saved observation remains intact, while
`ExecutionObservation` retains the actual failed settlement. The SDK does not
replace either fact with a successful receipt. Output received before an admission
rejection also requires protocol cleanup. The saved output remains evidence of
what was observed, and `ExecutionObservation` retains the original rejection
without inventing a provider result. Output received after rejection cannot be
added to that rejected invocation. After a rejected observation or reader failure,
confirmed cleanup (including an audit-only cleanup error) must unblock the backend execution future.
Agent continues polling and retains its actual result in `ExecutionObservation`
and saved settlement. With uncertain cleanup, it captures an already-ready result
but does not wait indefinitely for unavailable settlement; that case retains `None`.
It retains the accepted observation prefix and stops polling further chunks, so a
continuously ready stream cannot delay settlement or a later close. Cleanup errors
remain in the returned and saved local result.
Uncertain cleanup keeps admission unavailable until explicit close
confirms cleanup; `OperationAndCleanupFailure`
retains the original typed operation error alongside the cleanup error in the saved
result. Permission controls and close remain
available through another Agent clone while invocation waits. Explicit close and
automatic failure cleanup share one supervised backend shutdown attempt. Both
exclude new permission/steering controls and interrupt outstanding response waits
with `Closed`; backend ports retain admitted effects and audit until cleanup
settles. A dropped caller does not abandon an admitted permission operation.
An interrupted steering receipt retains uncertain delivery and is never replayed
as a queued prompt. Admission can reopen only after confirmed cleanup. Explicit
close also requires successful audit delivery: repeated close returns the retained
audit failure without inventing acknowledgement or running physical cleanup again.
The resource lease is released after physical confirmation even when audit failed;
audit failure alone does not keep a background cleanup loop alive.

The existing UI echo flow is still separate; wiring its authenticated commands to
Agent remains gateway integration work. Test providers live in tests. The
[Agent application tests](../../tests/application/agent_execution/agents.rs),
[storage tests](../../tests/infrastructure/session_storage.rs), and
[ACP Agent integration](../../tests/infrastructure/acp/contracts/agents.rs) exercise
persistence, failures, reconstruction, close/resume, and hook behavior without
calling a model.


Shutdown retains the first initiating cause and verified actor until resource
cleanup is confirmed. Explicit retries and automatic cleanup after the last Agent
handle is dropped use that same request; a later caller cannot relabel an ongoing
shutdown. Successful cleanup clears this ownership, and a resumed attachment can
record a new initiating cause. Stored observation IDs obey the same 256-byte tool
and review identifier limit as live controller admission, including custom storage.

Adapter errors are checked before SDK cloning and retention: each error tree is
limited to 1 MiB of allocated diagnostic payload and node storage, 128 error/hook
nodes, and 32 nested error levels. Spare string/vector capacity counts. Oversized leaf configuration, input, unsupported-operation, protocol, and transport
diagnostics keep their typed variant with compact text capped at 4 KiB and an
explicit truncation suffix. Storage-port Io/Corrupt diagnostics use the same cap.
Oversized composite
live evidence becomes `AgentError::DiagnosticLimit`; rejected trees are released
iteratively. This marker contains no reconstructed lifecycle state. Explicit
provider and observation reports retain the cause, known result, resource status,
and audit acknowledgement independently of diagnostic truncation. Admission never
depends on traversing an error tree.
Restored oversized evidence is rejected before provider restoration or snapshot
cloning; bounded diagnostics and explicit settlement facts round-trip separately. These are error
retention limits, separate from invocation and observation payload limits.

Local result failures preserve `InvocationRecord::local_outcome` when an earlier
local success was known. A later error changes the local status without erasing
that outcome or allowing contradictory completion evidence. Queued success requires
dispatch; cancellation of pending work remains a separate valid outcome.

Live text uses cached domain history and incremental byte/count accounting. Each
chunk checks its identity and ordering without rescanning prior turns. Consequential
changes invalidate that cache; saves and restoration validate the complete snapshot.

Provider readiness is separate from work admission. After confirmed cleanup,
permission controls remain unavailable while an invocation restores the provider.
Already-waiting controls stop too; only successful preparation enables controls
for the new provider generation. A retained audit failure prevents restoration
and remains visible instead of being replaced by a later physical confirmation.

If explicit close overtakes a saved direct invocation before provider dispatch,
`InvocationRecord::cancellation` retains the close cause and verified actor. Its
target is the enclosing request identity. It does not invent provider execution
or scheduling events; local failure and cancellation evidence remain distinct.

## Control delivery and stopping

Steering and permission controls use the same lifecycle owner. These facts have
separate meanings and must not replace one another:

| Fact | Owner | Meaning |
| --- | --- | --- |
| Admission and first stop | Work permit | Whether this work may start, and why its generation stopped. |
| Provider acknowledgement | Provider operation | What the provider confirmed, including steering injection. |
| Receipt validation and saving | Agent and SessionManager | Whether that result agrees with retained evidence and can be acknowledged locally. |
| Resource cleanup and audit delivery | Cleanup report | Whether resources were released and required audit was acknowledged. |

A stopped operation that has never been polled cannot start. For an operation
already started, a ready provider result is retained even when the stop notification
is also ready. A still-pending response wait is interrupted. Preparation remains
strictly fenced because it grants authority for new work, rather than acknowledging
an existing effect.

A permission receipt leaves the interruptible provider wait before local evidence
validation begins. Its work permit stays owned through validation. A concurrent
close cannot turn that received receipt into an unconfirmed delivery; contradictory
evidence still fails validation. Native steering similarly retains confirmed
injection rather than claiming the input was never delivered.

Cleanup decisions use provider state and work-permit evidence. Diagnostic labels
such as `Busy` or `Unsupported` do not establish whether the context is usable.
A stop applies only to the work identified by its permits; confirmed cleanup may
preserve waiting input for a restored context.


Joining cleanup keeps the original resource-cleanup request, but newly affected
waiting inputs receive the current close request and actor. Work already stopped
keeps its first cause. Waiting input survives automatic cleanup only when both
resource release and audit acknowledgement allow restoration. Failed audit stops
those owners, and the queue runner settles their receipts before exiting; a caller
does not need to close again to obtain their results.

Admission and attachment cleanup have separate timing. Close rejects new work
immediately. Its cleanup task then joins the same resource transition used by
restoration, so it cannot consume an old cached cleanup report while a replacement
attachment is being armed. The cleanup cause is captured after that handoff.
A delayed control result retains the attachment generation that admitted it. Its
result may still reach the caller after restoration, but its failure cannot change
the new attachment's state or start cleanup against that attachment.

Cleanup confirmation belongs to the provider attachment, not to one running
operation. An operation that confirms release records that fact with the attachment
owner immediately, so dropping the last Agent cannot close those resources again.
A stop may retire work while its final control result is arriving; confirmation
for the same attachment still contributes to the shared close report. Reports from
a previous attachment cannot change a restored attachment.

An in-progress close finishes collecting its own report before returning the
combined evidence. Confirmed release cannot be undone by a competing uncertain
result, and independent confirmation cannot erase failed audit delivery. Close
callers use the final combined report. A real cleanup retry on resources still
owned can acknowledge an earlier failed attempt.
