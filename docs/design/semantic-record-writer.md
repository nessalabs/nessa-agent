# Semantic record storage for conversations

**Status:** implementation in progress for [#275](https://github.com/nessalabs/nessa-agent/issues/275). This narrows [ADR 0009](../adr/todo/0009-reusable-event-stream-crate.md) to the first live write path. Nessa is in alpha, so the record store replaces JSONL for every conversation. There is no JSONL import, second reader, or handler version. An existing per-session JSONL file is refused before a stream is created; an old lock file alone does not imply history.

## What this changes

The SDK passes a complete observed `SessionSnapshot` and its ordered `SessionChange` decisions through `SessionStorageLease`. The record writer saves Nessa facts at the SDK decision that produces each fact. `event-stream` provides ordered, durable physical records and cursors; the SDK owns their meaning, command receipt lookup, and whether provider work may start. A phone reads committed facts through a later bounded record source. The provider never runs during replay.

Server composition opens one SQLite runtime before listening and injects the adapter through an SDK-owned port. The SDK retains its exclusive conversation lease for the life of the manager. A successful append returns the committed cursor. A timeout or dropped reply leaves the command unresolved until the same event ID and exact bytes are found or retried. The writer serializes each typed fact once and keeps those bytes across that reconciliation. Shutdown drains session owners before the shared runtime.

```mermaid
flowchart LR
    Command[Verified command] --> Manager[SDK session coordinator]
    Manager --> Fact[Typed Nessa fact]
    Fact --> Writer[Record writer adapter]
    Writer --> Stream[event-stream runtime]
    Stream --> SQLite[(SQLite store)]
    Stream --> Source[Bounded committed source]
    Source --> Fold[Shared Nessa fold]
    Fold --> Gateway[Gateway and device views]
```

## Commit boundaries

The SDK's current decision sites define the facts. A pending set is retained inside `Evidence` when observed state advances but a save has not been acknowledged. It contains exact typed changes in decision order; `flush_observed` commits those changes as one atomic logical fact when more than one change is pending. The writer does not infer lifecycle facts by comparing two arbitrary final snapshots. It checks that folding the pending changes from the generation's original committed state equals the observed candidate before attempting an append. A mismatch is typed corruption before any write or provider effect. The generation is scoped to the lease and is not encoded in physical records; replay starts a fresh generation sequence.

| SDK boundary | Fact | Before the next effect |
| --- | --- | --- |
| `prepare` creates an empty conversation | `session-open` with session and provider identity | Commit before provider open or input admission. |
| `prepare` clears saved queue membership after restart | `queue-decision` with restoration cause | Commit before new scheduling. |
| `attach` obtains a provider context | `provider-context` with previous and next context | Commit before treating that context as recoverable. |
| `begin_record` reserves an input | `input-accepted` with exact request, verified `ActionContext`, mode, pending receipt and first queued edge if present | Commit before queue membership, dispatch or acceptance reply. |
| Queue admission, selection, withdrawal, reorder | `queue-decision` with actor and scheduling prefix | Commit actual queue decision before any dependent dispatch. |
| `record_scheduling` | `scheduling-transition` with prior and next stage, cause, target and actor | Commit dispatch edge before polling normal provider work. |
| `event` receives provider output | `provider-observation` with owner and ordinal | Message fragments can remain live only until the next save boundary; durable views expose only committed observations. |
| Receipt acknowledgement changes | `receipt-updated` with previous and next value | Keep audit/storage acknowledgement separate from provider acceptance. |
| Undispatched cancellation | `stop-decision` with cause and caller | Do not infer provider cleanup. |
| `record_provider_report` | `provider-report` plus first dispatched local stop when applicable | Keep report and local stop in one fact. |
| `finish`, `retain_result` or queued failure | `local-settlement` with previous and next result and preserved earlier outcome | Do not replace provider evidence. |

A call that changes scheduling and queue membership together, or settles a failed queued input while removing it, commits one `atomic-transition` containing the ordered child facts. A later flush may include earlier message fragments or a receipt left observed after a failed write. Group validation applies all children to a candidate state and publishes it only after every child succeeds. Nested groups are invalid.

## State and ordering

| Current state | Event | Next state and action |
| --- | --- | --- |
| No stream | New conversation selected for records | Create stream, append `session-open`, then expose the conversation. A failure leaves it unopened. |
| Committed, no pending changes | SDK decision produces a change | Retain the typed change and observed candidate under the evidence mutex. |
| Pending changes | Save boundary | Encode the single fact or atomic group once; append stable physical records. Do not start dependent work yet. |
| Pending streaming messages | Eight MiB of retained message bytes or 1,024 message observations accumulate | Save the complete observed prefix before accepting more output. Each message is at most four MiB, so even sixfold JSON escaping plus event metadata stays below the 160 MiB fact limit. A failed flush retains the same generation and pending decisions; no later provider effect is authorized by that failure. |
| Oversized encoded atomic group | Preflight before installing a pending fact or advancing a save generation | Return a typed size refusal without an append or writer fence. The caller may retry a smaller valid group; an atomic decision group is never split without an explicit semantic boundary. |
| Appending | Confirmed complete fact | Fold and validate it, advance committed state and cursor, clear pending changes, then release the dependent effect. |
| Appending | Definite rejection | Keep pending facts and return typed refusal. Do not claim acceptance or dispatch. |
| Appending | Outcome unknown | Hold the affected command; read the same event ID and exact bytes. An identical committed record advances; changed bytes conflict; absence permits retry with identical bytes. |
| Multi-decision save | Earlier decision A commits, later decision B fails | Retain the caller's original observed candidate and exact ordered decision bytes. Track A as a physically committed prefix while the call remains unresolved. A retry with the same full sequence verifies and skips A, then resumes B; load cannot expose a partial call as settled. |
| Manager save, caller wait cancelled | Storage append commits after the caller drops its wait | The evidence keeps its save generation and pending decisions because the manager did not acknowledge success. The record lease retains a completed receipt for that generation. A retry validates the exact full decision sequence against its original base and returns success without appending those bytes again; only then does the manager clear pending and advance the generation. The canceled waiter starts no provider effect. |
| Manager save, caller wait cancelled | Append fails, remains uncertain, or the decision sequence grows | The evidence keeps the same generation and pending sequence. The writer retains its original base and physically committed prefix, reconciles any pending fact, then appends only the valid suffix. A changed or truncated prefix is refused before another append. |
| Manager save | A prior generation is complete and acknowledged | The next generation starts from the current committed state, even if its decision bytes equal the prior generation's bytes. A stale, skipped, or premature next generation is refused. The completed receipt is readable and does not fence erasure. |
| Fold `LocalSettlement`, including an intermediate change in an atomic group | A prior result exists and a new diagnostic or outcome arrives | Use shared application result validation and apply the same domain result transition as the live manager against the cached prior history before changing the candidate. A prior failure cannot become success; a prior success may become a diagnostic failure while retaining its known local outcome; another failure follows the domain rule. The supplied local outcome must equal the history's resulting outcome. Reject an invalid intermediate transition even if a later change would leave a valid final snapshot, before any append. The writer receipt handles exact physical retries of the same decision. |
| Fold `ReceiptUpdated`, including an intermediate change in an atomic group | A pending receipt becomes a failed or acknowledged receipt | Validate each proposed receipt with the same application rule used by restoration and live retention before replacing the prior value. A failed receipt names at least one bounded audit or storage failure. Reject an empty or oversized failed receipt before any physical append, even if a later decision would replace it with an acknowledged final snapshot; after refusal the previous history remains readable on reopen. |
| Fold initial admission and each invocation fact | A batch contains later changes that could mask an invalid earlier acknowledgement, dispatch edge, observation, report, stop, or local result | Build one prior `InvocationHistory` per affected execution and apply each domain transition in physical decision order before changing the candidate. Validate the admission acknowledgement and each receipt revision at its own boundary. A later scheduling edge cannot retroactively authorize earlier provider output or a provider report; scheduling after a local result must retain the domain's delivery prohibition. The final snapshot check remains the cross-record and complete-checkpoint authority. Cache the history per execution so large batches do not rebuild a growing history for every fact. |
| Fold a provider-context revision | An atomic group temporarily removes context and restores it after provider evidence | Track whether prior or newly appended facts need a provider context and validate each proposed context before replacing it. A later context revision cannot erase an invalid intermediate absence. Derive the flags once from the prior candidate and update them monotonically as facts append, avoiding repeated full-snapshot scans. |
| Fold an intermediate local diagnostic | An oversized failure is later replaced by a small failure in one atomic group | Validate retained error size at the shared local-result transition, before the first change mutates the candidate. Reject the entire group before append even when the final snapshot would contain only the small diagnostic. |
| Fold a provider report after a local success | The report's provider outcome agrees but its independent delivery or cleanup failure would make the public result fail, and a later local diagnostic replaces the success | Check the proposed report against the currently retained public result before mutation. Domain outcome agreement alone cannot validate the independent report failure; a later diagnostic cannot mask the contradiction. |
| Fold queue membership and dispatch | An atomic group orders queue admission before input, or Running before selection, but ends with a valid final queue history | Reuse the queue replay owner incrementally: each `QueueDecision` must find its input and exact scheduling checkpoint at the time it occurs and apply to the live pending queue; a Running edge requires a preceding selection for that execution. A later input or selection cannot repair a past decision. Rebuild queue authority once from the validated prior snapshot, then advance it once per queue fact; final queue replay still checks complete membership and checkpoint relationships. A valid grouped admission, selection, dispatch, result, and removal remains admissible in causal order. |
| Fold cross-invocation steering correlation | A steering input or injection names a queued target or an output offset produced only by later facts in the same group | At input admission, require the named target to be a prior invocation with established dispatch and require the captured target offset to equal its current observation count, as live admission records it. A targetless input has no offset. At an Injected edge, confirm the target's dispatch and captured offset still have prior evidence; never use a future output or dispatch to authorize an earlier steering fact. Keep final snapshot validation for whole-history consistency. Historical dispatch eligibility does not itself assert that a target remains active when a delayed acknowledgement is saved. |
| Fold context-bound provider observations | An atomic group changes context A to B, records a permission cancellation from A, then restores A | Validate each observation's session and invocation correlation against the context current at that fact, using the same helper as restored checkpoint validation. A later context revision cannot make an earlier cancellation valid. Keep full permission-request history validation after the group, but reject the mismatched cancellation before append so same-lease and reopened reads agree. |
| Writer opens or Reset installs a new stream incarnation | First save in that writer | Require the initial generation even when the physical history already contains earlier facts. Reset begins a new generation sequence for its replacement writer on the same exclusive lease. Reject a skipped first generation without changing the writer, so a valid initial save can still succeed. |
| Appending | Definite physical conflict or invalid frame | Fence this live writer's load and future saves as corruption. The prior snapshot cannot be exposed as proof that the conflicting suffix is absent. |
| Chunked fact, no matching seal | Same writer retries | Expose no part of that fact. The retained pending bytes resume the exact missing pieces and seal; no later fact may overtake it. |
| Chunked fact, no matching seal | Process restarts under the exclusive lease | Validate the fixed physical prefix and append a deterministic abort bound to the attempt start, declared digest and observed tail. If all body pieces are present, verify their digest before deciding the missing seal is abortable; a mismatch is corruption. Only after the abort commits may replay expose the last complete semantic state. No provider effect is replayed. |
| Abort append | Reply lost or process restarts | Retry the same abort ID and bytes against the fixed prefix. An exact committed abort advances the physical cursor without folding a semantic change; malformed, foreign or reordered evidence is corruption. |
| Aborted attempt | Caller retries the command | Derive a new physical attempt ID from the post-abort start offset while retaining the logical fact key. A changed command is checked against the last complete semantic state and admitted only by the normal SDK rules. |
| Reopen while abort is pending | Caller cancels open | Keep the exclusive reservation in the supervised recovery owner until the abort settles; a second opener cannot overlap it. |
| Committed acceptance, response lost | Same caller, request ID, execution ID and input retries | Return the original receipt; do not dispatch twice. A changed command under the same identity conflicts. |
| Provider accepted native steering but its acknowledgement was not saved | Restart | Preserve uncertain delivery. Check provider correlation if available; never inject again merely because the saved edge is absent. |
| Store failure during execution | Cleanup | Bound output and stop affected work; report only outcomes whose facts were committed. |
| Erase | Reset acknowledged, retired physical records remain | Retain cleanup ownership under the lease and run bounded global retired-record cleanup until no retired rows remain. Return success only after physical removal. Failure leaves erasure unresolved under the deletion tombstone. The same lease reconciles before load/save; after restart a retry recognizes an empty replacement by its physical stream bounds and drains cleanup without another reset. A live writer with unresolved decisions still resets even when the physical tail is empty. |
| Erase | Cleanup failed, process restarts before deletion retry | The durable gateway tombstone keeps product commands fenced. The adapter's new lease does not carry the old in-memory cleanup phase; the deletion owner retries `erase` to drain retired rows before marking the tombstone erased. A direct SDK caller must not treat an empty replacement snapshot as erasure acknowledgement. |
| Shutdown | New admission | Refuse new commands, join or cancel owners, save confirmable outcomes, drain writes, close runtime and release store. |

```mermaid
sequenceDiagram
    participant C as Client
    participant G as Gateway
    participant S as SDK coordinator
    participant R as Record runtime
    participant P as Provider
    C->>G: Send command and request ID
    G->>S: Verified caller and input
    S->>S: Reserve input and pending receipt
    S->>R: Append acceptance with stable ID
    R-->>S: Committed cursor
    S->>P: Start authorized work
    S-->>G: Accepted receipt
    G--xC: Reply lost
    C->>G: Retry exact command
    G->>S: Look up committed receipt
    S-->>G: Original receipt, no new dispatch
    G-->>C: Original receipt
```

```mermaid
sequenceDiagram
    participant S as SDK coordinator
    participant R as Record runtime
    participant D as Device reader
    S->>R: Append large fact start and digest
    loop Bounded pieces
        S->>R: Append next piece with stable ID
    end
    Note over D,R: Partial prefix is not a visible fact
    S->>R: Append matching seal
    R-->>S: Confirm seal cursor
    D->>R: Read committed records
    R-->>D: Start, pieces and seal
    D->>D: Validate and apply one fact
```

## Physical contract

The logical key is `(kind u8, execution ID length u16, execution ID UTF-8, ordinal u64)` in big-endian order. The physical event ID is `nessa-fact-` plus SHA-256 of that key and the attempt start offset, with a `-start`, `-part-N`, `-seal`, or `-abort-N` suffix. The start payload also carries that offset. The logical body is typed UTF-8 JSON using the existing snapshot field mappings for request, actor, observation, scheduling, queue, report, outcome and error values. The frame never authorizes a body merely because its digest matches; the application fold validates the typed fields and relationships.

The ordinal is the prior committed count for queue, scheduling and observation facts. Session open, input acceptance, stop and provider report use zero with their unique scope. Receipt, local settlement, provider-context revisions and atomic transitions use the previous committed physical cursor offset; a failed append cannot advance it. A retry therefore derives the same identity from the same committed state. The writer still compares exact encoded bytes before treating that identity as a duplicate.

An inline fact has one start record if the complete payload fits 64 KiB. Otherwise it has a start record, 64 KiB pieces and a seal. The encoded body limit is 160 MiB. The writer preflights that limit before installing a pending fact; an oversized indivisible group returns `TooLarge` without an append or a writer fence. Streaming messages flush at eight MiB retained bytes or 1,024 observations, well before JSON escaping can cross that limit. Direct-port partition of larger atomic groups is tracked in [#292](https://github.com/nessalabs/nessa-agent/issues/292). The adapter fixes the event-store record limit at 1 MiB, above its largest physical frame (a 64 KiB piece plus a nine-byte header). Physical IDs include the attempt start offset within the stream incarnation. Identical retries of the live writer use the same IDs and bytes; changed-byte reuse conflicts. A seal is the only visibility boundary for a chunked fact. On exclusive reopen, a strictly validated incomplete prefix can instead terminate with one durable abort record. The abort exposes no part of that fact. Framing lives in `crates/nessa-sdk/src/infrastructure/session_storage/stream_fact.rs` and is used by the record writer.

## Verification for #275

1. A real SQLite process restart reconstructs a new conversation with accepted input, retained output, settlement and receipt. Replay performs no provider effect.
2. An acknowledgement lost after a committed acceptance returns the original receipt on retry; dispatch count remains one. Change actor, request ID or input with the same execution ID and assert conflict before dispatch.
3. Inject definite rejection and uncertain append at start, piece and seal boundaries. Verify same-writer exact-byte retry, no partial fact exposure, no overtaking and no duplicated provider effect. After a child-process restart at each partial boundary, validate the prefix, durably abort it, and load only the prior complete state. Crash and retry during abort must produce one terminal abort and no provider effect. A malformed or foreign prefix remains corruption.
4. Hold two owners against one conversation, restart between writes, and exercise cancellation and cleanup while the store refuses writes. State the proven process-restart guarantee separately from power-loss durability.
5. Compare the folded committed state with the manager's committed snapshot at each save boundary. Exercise pending message fragments, receipt updates after failed writes, queue selection and removal, dispatched local cancellation, provider report and later local failure.
6. Reject a changed complete body with no seal before it can be aborted. Fail A, then B in an extended two-decision save, and verify exact full-sequence retry and unresolved load. Fail physical cleanup after Reset, restart, and verify the retired SQLite rows and original prompt payload are gone before erasure succeeds. At the gateway, a lost cleanup acknowledgement keeps the tombstone unfinished; a retry preserves the original actor, request and provider-erasure evidence.
7. Cancel a manager flush after the SQLite fact commits but before its reply reaches the caller. Pending evidence retains the same lease generation; a later flush acknowledges the completed receipt without appending a duplicate fact. A second equal observation advances to a distinct generation and is saved separately. Exercise a canceled wait before commit, an extended same-generation sequence, changed/truncated prefixes, stale/skipped generations, and an incomplete batch refusing the next generation. Keep the original bounded close and panic-attribution conformance tests passing. Restart a gateway after an incomplete erase and verify its tombstone refuses product commands until deletion retry finishes.
8. Submit local-result revisions through the public record lease: failure to success and changed success are refused before append; success to diagnostic failure retains the known outcome, and failure to another failure is allowed. A two-decision batch whose first revision is invalid must be refused even when its final candidate equals the prior valid snapshot. Reopen to verify only accepted revisions replay.

## Bounded read source for #276

The read source shares the server's already opened event runtime. It never acquires
the SDK writer lease, creates a stream, or repairs a partial write. A source is
bound to one existing stream incarnation and the host's origin identity. It
maps each physical event to one dense sync position. Its payload carries the
physical schema tag and exact frame bytes; the semantic receiver maintains an
applied fact cursor separately from the downloaded physical checkpoint.

| Current source state | Event or ordering | Result |
| --- | --- | --- |
| Existing stream, validated terminal at zero | Head read during a complete append | Scan in physical order to the last validated inline start, seal, or abort; return that terminal position. |
| Validated terminal before an incomplete start or pieces | Head read before seal | Return the prior terminal; a page cannot expose the incomplete suffix. |
| Incomplete suffix | Matching seal commits after the head read | A later head read validates the full attempt and advances to its seal. The earlier fixed target stays unchanged. |
| Incomplete suffix | Writer or recovery commits a matching abort | A later head read advances through the abort without producing a semantic fact. |
| Captured terminal head | Page read while the writer appends | Return a contiguous bounded prefix no later than the captured target; later writes do not change that page's target. |
| Reader is catching up while the writer keeps appending | A head request observes a physical tail, then newer facts commit before each read | Validate only through the tail captured for that request and return the last terminal at or before it. The next head request may advance. Other readers must not wait for a moving tail to become idle. |
| Reader has a historical fixed target | Newer facts commit before or during its page request | Validate the requested terminal through that target only; do not first catch the shared worker up to the current tail. Return the bounded page or a typed invalid-target result. |
| Captured terminal head | Reply drops after receiver commit | Receiver reloads its durable physical checkpoint; repeated pages have the same IDs and bytes. Semantic application remains atomic with its separate applied cursor. |
| Reader bound to an incarnation | Reset or deletion replaces the stream | Refuse the old scope with identity-changed; a new incarnation needs an explicit receiver reset. |
| Reader bound to an incarnation | Wrong origin, stream, incarnation or schema, a gap, malformed frame, or a target inside a fact | Return a typed refusal before exposing an unvalidated page. |
| Read queued on the source worker | Caller drops its wait or SDK shuts down | The owned worker finishes or reports unavailable; it does not mutate storage or take over writer recovery. |
| Shared source worker is busy | Authorized clients submit more reads than its 64-command queue can retain | Admit at most 64 waiting commands; return `SourceError::Unavailable` immediately for the excess request, with no source read or allocated backlog. A client may retry after backoff. When the last source drops, the worker drains admitted commands and exits even if the queue was full. |
| Repeated authorized reads on the same source | More head and page requests arrive | Clones share one worker and its last validated terminal. A repeated head checks for new records from that point. An old fixed target outside the bounded remembered-head set is revalidated from the stream prefix before paging. |
| Receiver has a downloaded physical prefix ending inside a chunked fact | Receiver process restarts | Reload the durable physical checkpoint and staged frames; leave the semantic applied cursor at the prior terminal. Continue from the physical checkpoint, then validate the seal and atomically advance the applied cursor with its projection. |
| Receiver commits a page and projection but its reply is lost | Receiver process restarts | Reload both cursors and the projected state from the same local transaction. Rechecking the source starts after the downloaded checkpoint; do not apply the prior fact again. |
| Receiver receives a foreign scope, position gap, changed frame schema, or invalid seal | Before local transaction | Refuse the page or semantic projection with typed failure. Keep both durable cursors and the prior projection unchanged. |

The development loopback endpoint from sync-engine serves authorized head and
page reads. It has no wake subscription. A receiver catches a write that lands
between a captured head and its idle decision by rechecking the head after the
fixed-target pass, then polling or reconnecting after a failed check. Push wake
delivery and production authorization/routes remain [#260](https://github.com/nessalabs/nessa-agent/issues/260)
work; this slice makes no subsecond delivery claim. The loopback test reports
actual framed transport payload and protocol bytes, including retries.
