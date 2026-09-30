# Agent entry point

Construct one `Agent` for a conversation. Give it an execution provider and a
`SessionManager`, register hooks, then submit messages. The public API is the Rust SDK;
the TypeScript gateway client does not yet expose agent chat RPCs.

```rust,ignore
let provider = Arc::new(ClaudeAcpProvider::new(config, &model, limits, audit)?);
let storage = Arc::new(RecordStorage::new("./sessions")?);
storage.initialize().await?;
let clock = Arc::new(RuntimeMessageCommitClock::new());
let manager = SessionManager::open(None, storage, clock).await?; // Fresh local UUID v4.
let session_id = manager.id().clone(); // Retain this key to reopen the conversation.
let agent = Agent::prepare(provider, manager, audit.clone()).await?;
let authorization = agent.authorize_attachment(AttachmentRequest::CallerRequested(verified_actor.clone()))?;
agent.start_attachment(authorization)?.wait().await?;

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

Omit the local key with `SessionManager::open(None, storage, clock)` to create a fresh
UUID v4 session. For a named or existing session, pass `Some(SessionId::new("test-session")?)`.
Save `manager.id().clone()` to reopen a generated session; `None` always creates a new key.
This session UUID identifies the conversation. Each new logical submission also
needs a separate `ExecutionId`; retries preserve that submission ID. The SDK does
not generate execution IDs automatically.

## Ownership and call flow

```text
host / future chat RPC
    -> SessionManager::open(optional local SessionId, storage, monotonic clock)
        -> SessionStorage::open -> SessionStorageLease [exclusive writer lease]
    -> Agent::prepare(provider, session_manager, audit)
        -> load saved snapshot and verify provider identity without provider I/O
    -> authorize_attachment(attributed request) -> Waiting
    -> start_attachment(single-use authorization) -> Starting
        -> AgentProvider::open(saved provider context or None)
        -> save attached context identity -> Attached
    -> Agent::add_hook(event, callback)
    -> Agent::enqueue(request, actor)
        -> capability validation + saved queue admission
        -> wait for the single invocation slot
        -> save submitted input and caller attribution
        -> before hooks
        -> provider prepares/restores its live context
        -> provider executes while Agent drains observations
        -> stream text; save at cadence or tool/review/terminal boundaries
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

`agent.operation_capabilities()` returns a separate effective
`OperationCapabilities` snapshot. Provider adapters publish raw
`ProviderOperationCapabilities`; `ProviderSession` resolves that input so a
provider cannot enable an application feature whose common integration does not
exist. The effective type has private fields and scoped accessors, which prevents
constructing combinations such as provider permission denial implying a configured
pre-tool policy evaluator.

ACP publishes its provider facts only after successful connection and session
verification, and resets them before restoration. Until negotiation completes,
provider-derived feature facts are `Unknown`. The existing boolean steering,
resume, and image fields retain their negotiated flag for the same boundary.
Closing retains the last negotiation. Permission denial means only that Nessa can
select a rejecting option from a provider-raised review that offers one. Explicit
permission deferral is a distinct provider outcome; ordinary waiting for a user
answer is not deferral. Raw hook suppression, provider compaction notices,
provider model-switch notices, and provider elicitation forwarding remain unknown
until an adapter proves their full scoped behavior.

The effective snapshot reports compaction and model-switch reporting, explicit
permission deferral, configured pre-tool policy enforcement, policy end-turn, and
policy session-close as `UnsupportedNotImplemented`. The SDK has no correlated
application event or transition for those operations. Incoming agent questions
have a correlated application path. Their support is `Unknown` until connection
negotiation, `SupportedWithCorrelatedRoundTrip` for a verified question-capable
binding such as the pinned Claude ACP profile with tools enabled, and `Unsupported` for bindings
that do not offer questions, including the current Codex and Opencode profiles.
This concerns agent-originated form questions, not MCP elicitation forwarding.

Support is a UI hint, not an admission permit: a supported operation can still
fail because the target finished, the connection failed, or saved history is
unavailable. Read the snapshot again after reconnection. SDK-owned queueing,
next-invocation steering, and invocation hooks remain available independently of
native provider support. No provider model/tool-step hooks are implied.

### Reasoning effort

The model's catalog effort levels (`agent.capabilities().effort_levels()`) are a
ceiling. Once a connection is negotiated, `operation_capabilities().effort_levels()`
says which of them the connected agent also offers: the levels its ACP
`thought_level` config option lists, matched by exact name, in catalog order. It
never names a level the catalog lacks, such as Claude's own `default` or a level
Codex offers past `max`. `agent.effort_levels()` reads the two together as
levels, and is `None` before negotiation, while restoring, and wherever the
model, binding, or agent offers none (Claude on Haiku 4.5 lists no effort option).

The Claude and Codex bindings select a level for every session they open with
`with_effort_level`; without one nothing is sent and the agent keeps its own
default. `agent.set_effort_level(level, actor)` changes it on an idle attachment,
under the same scheduler lock as `set_approval_mode`. A change that reaches the
agent is audited as requested, then applied, refused or failed
(`ExecutionAuditRecord::EffortLevelChanged`, both records made from one
`EffortLevelChange` with the caller and both levels), and succeeds only once
both records are accepted. It runs to its settlement on a task of its own:
dropping the caller's future does not leave a request without its outcome. Nothing is sent when a turn is
queued or running (`Busy`), when nothing is attached or the connection is not
negotiated yet (`AttachmentUnavailable`), or when the level is not offered
(`InvalidInput`). The agent's reported level must match the one selected, at
open and after a change; a mismatch is a protocol failure. A connection the same
attachment restores selects the last verified level again; a new attachment
starts at the binding's level. `agent.effort_level()` is the level in force: the
last one verified on the current attachment, or else the binding's, including
while detached. Every queued admission records it as of admission
(`QueueAdmissionRecord::effort_level`); a turn still queued when its attachment
is replaced runs at the new attachment's level
([#313](https://github.com/nessalabs/nessa-agent/issues/313)).

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
- `RecordStorage` stores typed decisions in a shared SQLite event runtime and
  holds an exclusive writer lease per conversation. Initialize it before
  accepting conversations and drain it after the agents stop. Create independent
  session IDs for separate chats.
- `RecordStorage::record_source` opens a reader over an existing conversation
  stream without taking that lease. Clone the source for separate authorized
  requests so they share its validated head and one worker. Its sync-engine
  `RecordSource` methods run on that worker and must be invoked from a blocking host thread. The
  head stops at the last validated inline fact, seal, or abort. Pages carry exact
  physical frame bytes under a schema tag and are limited to 64 records and
  512 KiB total payload. Downloaded physical progress does not itself mean a
  semantic fact has been applied. A reset changes the incarnation and refuses
  the old source; receiver reset and authorization belong to their host owners.
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

`Agent::prepare` returns `AgentInitializationError` only for local identity, validation, and storage preparation failures; it performs no provider I/O. Provider opening belongs to the generation-bound attachment task. `AttachmentWait` reports its provider, audit, storage, and cleanup outcome, while `attachment_status()` atomically projects phase with its matching bounded failure. Closing or dropping an unused authorization fences that exact generation and wakes its cancellation wait. Provider open failures retain cleanup ownership inside the Agent until cleanup is confirmed.

Dropping a recovery error with an available cleanup handle delegates cleanup to
a task with bounded backoff. Adapter panics while constructing, polling, or
dropping a cleanup future leave cleanup unconfirmed and retain the same target
for retry.
Keep the Tokio runtime alive until cleanup finishes; shutting down that runtime
cannot establish that an external process stopped. If it cancels the last cleanup
task, the SDK keeps the storage lease fenced for the process lifetime. Await explicit cleanup before
host exit. Audit-only failure remains reported separately from confirmed resource
cleanup.

Streaming text and thoughts reach subscribers immediately. The first unsaved
message sets a fixed 100 ms deadline; 16 KiB of message payload or 64 message
observations cause an earlier save. Tool/review events, terminal observations,
and invocation settlement save accumulated observations immediately. The injected
monotonic clock wakes the active invocation supervisor even when the provider is
silent. If the process fails mid-stream, text since the last save may be lost;
recovery never reruns the provider to recover it. Exact chunks and their order are
preserved when saved. Admissions and consequential lifecycle changes still save
before successful acknowledgement. Mandatory permission audit remains separate.

The record adapter saves typed SDK decisions in one SQLite stream per session.
Each complete fact folds into the restored snapshot without invoking the provider.
An incomplete fact retains its exact bytes for retry by the live writer. After a
restart, a validated incomplete tail is durably aborted under the exclusive
lease before restoration; a malformed tail is refused as corruption. A
pre-existing per-session JSONL history is refused unchanged before a stream is
created.

Each lease save carries a generation owned by the session manager. If a caller
stops waiting after a physical write, the manager retains its pending decisions
and generation. A retry with that generation verifies the completed record and
acknowledges it without a second append; a valid added suffix continues from
the committed prefix. The manager advances the generation only when it clears
pending evidence after a successful save. Later equal observations use separate
generations and are stored separately. A record writer fences an incomplete
generation, while a completed receipt remains readable and erasable. A fresh
writer starts at the initial generation, including the replacement stream
incarnation installed by Reset on the same lease.

`SessionStorageLease::erase` resets the session stream under its exclusive lease
and drains retired physical records before reporting success. An uncertain reset
or cleanup is reconciled before another load or save on that live lease. After
a restart, the gateway's deletion tombstone bars product commands and retries
erase before reporting completion; an empty replacement snapshot alone is not
an erasure acknowledgement. Erasure does not
retire the identity; a later semantic save begins a new history. It does not touch
the provider's own record of its context.

A caller that only means to read or erase what a session saved takes its lease
with `SessionStorage::open_existing`, which answers `None` without creating a
session stream for an identity that was never opened. What it
reads it reads through `SessionSnapshot::load_saved`, which applies the checks
restoration applies (every relationship in the snapshot, and that it was saved
under the session asked for) so a custom adapter cannot hand it another
session's history. The provider identity is not compared there; only a caller
holding the configured provider can, and restoration still does.

The provider's own record is reached separately. `ProviderSessionDeleter` is
implemented by each agent binding in its own module, over one shared ACP
exchange: a connection of its own, launched as the provider launches one,
`initialize`, and `session/delete { sessionId }` only when the agent advertised
`agentCapabilities.sessionCapabilities.delete` — the session is never loaded or
resumed. Each binding says what a successful answer means for its agent:
`Deleted` for Claude, whose adapter deletes the session file; `Archived` for
Codex, whose adapter archives the thread; `Acknowledged` for Opencode, where
nothing more is known. An agent that does not advertise deletion answers
`NotSupported` without being asked.

The delete is sent first whenever the agent can be asked, and an accepted
delete depends on nothing else: only the agent knows whether it still has the
session, and an agent may leave one out of its list (Claude's lists no session
without a titled prompt, so a conversation of images alone is never listed) or
keep it under a workspace since changed. Only a refusal is read against the
list, when the agent advertises `sessionCapabilities.list`: its list for the
configured workspace — followed through `nextCursor` for at most 64 pages,
within a second startup budget, and read with bounds of its own (16 MiB, about
a million values), since an agent may send its whole list in one frame. The
agent's own error answer to deleting a session its list, read in full, does not
name settles as `NotListed`: that is what an agent says of a session it no
longer has, and it is what lets a deletion interrupted after the agent deleted,
but before the host wrote that down, finish. Any other refusal is returned as
it is, `AgentError::Provider`, and asked again: the list names the session, or
could not be read in full — refused, past its budget, too large, a page without
`sessions`, an entry without a string `sessionId`, a `nextCursor` that is
neither a string nor null or repeats one already followed, too many pages. The list's own
failure is never what is returned: `AgentError::Provider` is only ever the
agent's error answer to `initialize` or to the delete — a refusal of the delete
once `initialize` succeeded — never to the list. No error's text is read.

A caller that stops waiting ends the exchange; the binding still stops the
connection's process and releases what its launch made, on a task of its own.
`ProviderSessionDeleter::settled` resolves once those tasks are done, or their
stop budget (`shutdown_grace` + 4 × `kill_timeout`) has passed; a host awaits it
before its runtime ends, since a runtime that ends drops such a task mid-cleanup.

The delete's budgets — `launch_timeout` for `initialize`, `startup_timeout`
for the delete, and one more `startup_timeout` for the whole list — are
moments on `AcpConfig::clock`, as every ACP deadline is (see
[transport](transport.md)); its tests move that clock themselves, so no answer
they check depends on how fast the machine is.

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
The record adapter is the server's durable session path. Permission answers and cancellations use the
required independent audit sink. It retains answer selection, response-write
observations, cancellation causes, and once-only live session closure (including idle closure). Snapshots do not replace that audit or
claim to capture every permission decision and its authorization evidence.

No record-writer performance workload has been qualified yet. Report measured
save, replay, CPU, and memory costs only with the workload and environment that
produced them.

## Committed transcript receivers

`infrastructure::session_storage::TranscriptFold` applies the next contiguous
physical records for one exact sync scope. The receiver journal owns duplicate
byte checking and downloaded progress D; the fold owns semantic decisions and
applied terminal A. Pending physical pieces can leave D greater than A. Source
freshness is separate from semantic completeness: accepted records do not confirm
Current. The caller authenticates and correlates a physical source-head response,
then calls `observe_source_head`; unavailable source evidence remains Unknown.
A restored checkpoint is Stale until that observation, and it cannot bootstrap
arbitrary recent tail records without the preceding checkpoint or prefix.

Use `transaction()` when receiving data, checkpoint, A and local effects must
commit together. Its borrowed guard stages `apply`, `observe_source_head` and
`confirm_empty`; immutable methods expose the staged checkpoint and positions.
Dropping it, including unwind or an external SQL/audit error, rolls back staged
semantic and physical evidence. A rejected operation rolls back the whole guard
and disables commit. Commit on an active guard adds no validation, new typed
refusal or source observation; the caller confirms its physical transaction first.
It releases moved undo values. Their existing destructors can allocate bounded
teardown work, including the shared typed diagnostic tree owner.
The guard performs no I/O, provider action or source reservation. Its exclusive
borrow prevents another fold mutation during staging.

The canonical continuation reuses semantic indices and validation state and
undoes touched suffix components. It retains full history and derived allocation
state; copied prefixes are unnecessary for receiver staging. Explicit `Clone`
still copies/revalidates full history. A read publication materializes a separate
immutable full snapshot in O(history), so repeated read results promise equal
semantic values rather than Arc pointer identity. Checkpoint encoding also visits
full history and retains full encoded output, using immutable bounded chunks and
borrowed message text. Nested DTO working values follow existing per-value limits.
Neither the cache eviction target nor one checkpoint chunk limits total valid
conversation history. Undo working allocations follow the caller's batch size and
touched component limits; public arbitrarily large batches do not promise constant
memory. See the compiled transaction example on `TranscriptFold::transaction`.

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
Provider failures retain a separate `ProviderDiagnostic` of at most 4 KiB when
the provider supplied text; their numeric code remains the typed decision fact.
Provider diagnostic prose has no admission, settlement, or cleanup authority.
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
