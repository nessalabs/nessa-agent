# Committed transcript fold (#277)

`sessions::records::fold_changes` owns lifecycle decisions. The inward
`CommittedTranscript` publishes the complete validated semantic snapshot and
applied position together. The infrastructure `TranscriptFold` is an effect-free
physical receiver: it checks source scope and physical framing, decodes a complete
fact, and asks that owner to apply it. It has no agent, provider, tool, audit, or
command dispatcher.

A receiver stores downloaded records and D in its journal. Its checkpoint contains
exact receiver, origin, stream, incarnation, schema and access epoch, terminal A,
fact count and the full semantic snapshot. It contains no downloaded record
journal or pending physical payload. Restore receives the receiver's independently
stored A, verifies agreement, applies existing snapshot allocation preflight and
semantic validation, and starts downloaded progress at A. The receiver drains its
stored A..D suffix before fetching a new page. The checkpoint and A belong in one
receiver transaction. This is validated continuation from a trusted local receiver
store, not authentication of arbitrary checkpoints against an origin.

Checkpoint encoding streams immutable chunks through the existing semantic codec.
Token/collection preflight and full state validation enforce the semantic owner's
bounds without an unrelated aggregate history byte cap.
Semantic history is not truncated to fit display or checkpoint limits. Staged
batches share prior immutable semantic state; physical history is not cloned or
retained after a terminal. Pending physical data is bounded by the physical fact
codec and is decoded once after its seal.

Completeness and freshness have one inward owner, `CommittedStatus`. Partial
completeness remains observable when freshness is stale or unknown. The coarse
product `transcriptState` derives from it and gives stale/unknown precedence.
`CommittedSession` has private fields and validates session identity/incarnation, positions, status and
full snapshot together. The gateway checks the returned session identity before
passing that immutable validated result to projection replacement, then rechecks typed metadata access after its async read
and title lookup.

The bounded display contract is the recent24 message records plus interaction-only
rendering of the exact active record when it is outside that window. The extra
record contributes no message, prompt, attachment or output copy. Messages remain
at most24. The60000-byte budget bounds the serialized application transcript
body after title lookup and authority filtering. Internal selection is skipped by
that serializer; the product wire adds catalog approval presets and runtime display
metadata afterward. Those additions are outside this display budget. This is not
a64KiB wire guarantee; transport uses its separately configured frame limit. Every replacement rebuilds renderer tombstones from its current
validated snapshot.

The gateway renders transcript, input, receipts, terminal status and interactions
from committed state. Broadcasts and receipt callbacks have no transcript write
path. Exact local active execution identity separately identifies which unfinished
committed execution this process can answer for; it may select running versus
unresolved display and actionable interactions, but supplies no terminal result,
receipt, completeness or applied position. Runtime capability and lifecycle fields
come from Agent. Deletion comes from the metadata tombstone. A different execution
or incarnation grants no interaction authority.

| State | Input | Decision / regression |
| --- | --- | --- |
| Empty | No source read | Not loaded; no complete-empty claim. `empty_and_unread_are_distinct_and_scope_is_exact` |
| Empty | Confirmed zero head | Complete empty; no semantic session. Same test. |
| At terminal | Next inline or sealed fact | Fold once and publish candidate+A atomically. `checkpoint_suffix_matches_full_replay_and_rejects_stale_scope` |
| At terminal | Unsealed start/pieces | Advance D only. `partial_and_abort_advance_physical_progress_without_semantic_change` |
| Partial | Matching seal | Decode complete body and publish candidate+A. Separate receiver restart fixture. |
| Partial | Matching abort | Clear pending attempt, advance A, keep semantic snapshot. Partial/abort test. |
| Any | Duplicate downloaded page | Receiver store compares saved bytes; fold refuses stale suffix without mutation. Duplicate/gap test and separate receiver lost-apply fixture. |
| Any | Foreign scope, gap, unknown schema, malformed seal, invalid intermediate decision | Reject batch without publishing candidate or progress. Duplicate/gap/invalid decision test. |
| Partial | Newer head not yet reached or unavailable source | Preserve partial completeness with stale/unknown freshness. Partial/abort test. |
| Checkpoint | Restart with D>A | Validate full semantic state and independently saved A; drain stored suffix A..D. `durable_receiver_restarts_mid_fact_and_after_lost_apply_reply` |
| Checkpoint | Altered scope, cursor, state or allocation overflow | Refuse before publication; checkpoint validation tests. |
| Newer projection | Older complete read arrives after partial progress | Refuse lower D even when A equals. `committed_partial_progress_cannot_be_replaced_by_an_older_complete_read` |
| Pending physical body | Stage each page, malformed suffix, shared-tail drop and Complete/Abort | Reuse the sole FrameValidator; immutable Arc piece chain clones its tail in O(1), charges node/control-block/payload allocations, and never copies accumulated prefix while staging. Complete assembles once and invokes the existing semantic decoder once; Abort drops without decoding. Iterative teardown preserves shared tails. `pending_piece_staging_shares_prefix_and_decodes_only_at_seal` and pending buffer operation-count tests. |
| Cached continuation | Concurrent reads | One per-conversation mutex advances the exact scoped continuation; snapshots share immutable state. |
| Cache | Eviction target is exceeded | Evict inactive entries; retain fixed-pass progress and account pinned intentional history separately. Never truncate semantic state. |
| Gateway read | Metadata tombstone commits during title lookup, normal or pending-mode read | Recheck the authoritative metadata after the await and refuse publication. `committed_reads_recheck_tombstone_after_title_lookup` |
| Incomplete view | Agent capabilities refresh | Queue, steering and interaction controls remain unavailable. `incomplete_committed_view_suppresses_controls_even_after_capability_refresh` |
| Exact active record outside recent display window | More than24 later queuedinputs; consumption/closure/replacement | Render only its interactions from current validated facts without copying message/output; preserve all actual question siblings, one identity per interaction, and no old tombstoneledger. Large escaped asks and multiple complete reviews use the existing60000encoded-byte owner; reviews yield atomically with the display-limit notice while fitting asks remain visible. `active_interactions_survive_recent_message_window_without_copying_the_message` |
| Valid custom-backend asks | Aggregate exceeds ACP-only room or encoded view budget | Preserve complete SDK history and status; after other content/reviews yield, omit only whole question units until actual encoded view fits. Mark truncated and publish one generic pending-interactions interactionViewError notice with actual Stop guidance, preserving the meaning when both reviews and asks yield. No ACP admission policy reconstructed from retained history. `custom_backend_questions_over_display_budget_remain_semantic_and_atomic`; browser fixture preserves3real sibling field selections through the conditional interaction notice. |
| Immutable ask collections | Caller provides large spare Vec capacity | Existing Question/AgentQuestion constructors retain compact boxed slices; actual retained event accounting includes owned collection slots and text. `ask_collections_compact_spare_capacity_and_account_owned_allocations` |
| Cache discovery | Old or missing lookup completes after a new incarnation is admitted | A lookup never orders UUIDs or retires other keys. Store the actual StreamKey; only typed runtime invalidity retires that exact entry, acquiring the existing cache publication mutex only after metadata I/O so retirement and publication share one fence. Recheck the exact key after source join before publication. At capacity, inspect at most 64 existing entries outside the cache lock; transient metadata failures cannot retire. `late_discovery_cannot_retire_current_receiver` covers old/missing late and opposite valid orders, retained ownership and eventual new progress. |
| Accepted physical suffix | Prior freshness Current, Stale or Unknown; partial, terminal or abort | Acceptance establishes D/completeness only. Current becomes Unknown; Stale/Unknown remain. Even reaching a previously observed head needs another explicit exact observe_source_head to establish Current. `accepted_suffix_cannot_confirm_source_freshness` |
| Combined shutdown failure | Real worker panic plus unresolved runtime; nested custom or restored errors | Preserve typed read/runtime children in an immutable boxed payload and strict typed codec. Reuse the existing error-tree node/depth/allocation budgets; keep 4096 diagnostic text admission separate. Reject hostile tree construction with a typed diagnostic resource outcome before retention/clone, and tear down iteratively. Real owner children remain admissible; no cause is stringified or discarded. Typed combined/single outcomes, exact limits and hostile codec/drop regressions cover this row. |
| Restored shutdown error | Malformed, foreign string codec, or over-budget nested causes | The existing structural/text preflight uses published error-tree budgets, then fallible typed decoding propagates refusal through saved acknowledgements, results and settlement. No DiagnosticLimit substitution becomes historical evidence; valid typed children survive and refused application leaves prior snapshot/cursor unchanged. |
| Storage diagnostic role | Same large plain Io cause appears inside AgentError, acknowledgement storage, or shutdown aggregate | Plain SavedError::Storage leaves consume the existing enclosing AgentError tree byte budget. Failed-ack storage and ShutdownFailures payload entry establish the published4096 text budget; nested children share it without reset and share the same node/depth owner. More-than4096 AgentError text remains exact through codec/fold; the same text in ack/aggregate roles refuses before allocation/publication. |
| Source joined | Exact key remains valid but sealed or mid-fact physical suffix appended | Observe the actual bounds physical tail through observe_source_head, then construct the result from that same physical observed head. A remains the terminal semantic cursor, fixed captured pass stays unchanged, completeness is independent of freshness, and the next pass makes finite progress. |
| Final returned ConversationView | Normal title lookup and pending-mode selection/capabilities follow pure projection | Apply the existing encoded display budget once after all service additions and authority filtering, before publication. Pure projection is internal and may be unbounded; preserve access recheck and whole-interaction fallback. Near-cap title regressions exercise both service returns while retaining pending selection. Selection is internal metadata skipped by this serializer; the product wire joins catalog fields afterward. `returned_committed_views_bound_late_titles_and_preserve_pending_selection` |
| Immutable semantic snapshot retention | Some snapshot versus empty continuation, staging clone and abort | CommittedTranscript counts the full snapshot once plus its two Arc control slots only when Some; staging shares the immutable allocation, abort preserves it. No new logical admission limit. `semantic_snapshot_retention_counts_arc_control_slots_once` |
| Storage failure allocation | Snapshot acknowledgement versus Agent error wrapper | One StorageError allocation_bytes owner counts String capacity or boxed two-cause payload/text; existing 4096 diagnostic admission meaning stays unchanged; the current codec stores typed read/runtime children. `shutdown_failure_allocation_accounting_agrees_across_wrappers` |
| Committed interaction | Foreign/local execution identity | Only exact live execution can offer it. `only_the_exact_live_execution_can_offer_a_committed_interaction` |
| Fixed read target | Writer appends suffix | Finish captured target; a later read captures a new head. |
| Restored semantic state at A=D | Authenticated correlated matching full scope/head | Public observe_source_head confirms Current; zero confirms loaded empty. This is a read snapshot, not a reservation. `observed_source_head_owns_restored_freshness_and_scope_agreement` |
| Restored or partial receiver | Newer authenticated head / unavailable source | Newer head marks Stale, unavailable mark_unknown; completeness remains independent. Same regression. |
| Receiver with saved D | Any foreign scope component or head below D | Refuse without mutation before publication. Same regression. |


Former callback-only text and receipt outcomes intentionally do not appear until
their semantic records commit. `gateway_record_view_waits_for_message_commit`
proves that difference. Display clipping is independently tested against JSON
escaping and ordered message identities; it does not alter receiver continuation.

### Read and shutdown ownership

One storage admission owner closes writer reservations, runtime initialization and
committed reads together. It retains at most four outer read-thread handles until
joining them. Each outer thread runs outside Tokio's entered context, owns its
source through source-worker join, and sends only a receipt to the async caller.
Cancellation drops that receipt; it does not release the admission slot. Cached
continuations have one exclusive fold owner per exact scope; callers clone only
its Arc and immutable published snapshot. Shutdown has one shared completion
covering both read joins and runtime cleanup, including cancelled callers.

| State | Ordering | Decision / regression |
| --- | --- | --- |
| Four admitted reads | Fifth read before any join | Typed capacity refusal; no source or fold cloned. `read_admission_remains_bounded_until_join` |
| Admitted read | Caller cancellation then gated source finishes | Slot held until outer/source join; shutdown waits. `cancelled_read_is_drained_before_shutdown_completes`, `storage_shutdown_waits_for_source_join_after_caller_cancellation` |
| Open storage | Shutdown races writer/read/init admission | One lock closes every admission; prior initialization is awaited before runtime cleanup. `shutdown_closes_all_admission_and_shares_completion` |
| Closing storage | First shutdown caller cancels; second arrives | Shared owned task completes full cleanup; same outcome for both callers. Same regression. |
| Read join and runtime cleanup | Both fail | Preserve both diagnostic causes in one immutable boxed shutdown payload, compacting retained allocations; codec retains the same read/runtime fields and outcome meaning. `shutdown_preserves_read_and_runtime_failures` |
| Cached exact scope | Concurrent reads | Exclusive continuation owner advances once; hot results share immutable state. `independent_committed_read_uses_physical_facts_while_a_writer_exists` |

### Long history and bounded work

The cache's 256 MiB target admits/evicts inactive receivers; it does not cap valid
semantic history. Each exact receiver has an exclusive mutex and one fixed raw
pass target retained across calls. Its continuation is pinned while D is below
that target, including when semantic state or a pending fact exceeds the cache
target. After the captured raw tail is reached (including a torn semantic tail),
or a typed refusal, the inactive receiver is eligible for whole-entry eviction.
Other conversations do not wait on its source I/O under a global cache lock.
Four read owners and 64 receiver entries bound concurrency/count; retained
intentional history is measured separately from codec-bounded pending work and
page/decode scratch. No valid history is truncated to fit a cache target.

A checkpoint is one opaque ordered collection of immutable byte chunks, each
bounded before retention. A chunk-chain Reader feeds the existing token preflight
and full semantic state codec/validator. The encoder streams existing field codecs
into chunks, producing at most one bounded field value at a time; there is no whole
snapshot DTO clone or aggregate JSON Vec. Valid aggregate state follows existing
semantic collection/retention owners. Scope and independently persisted A are
checked before returning restored state. The receiver atomically stores the whole
ordered checkpoint with A; raw downloaded frames remain its separate journal.

| State | Ordering | Decision / regression |
| --- | --- | --- |
| Pinned continuation near cache target | Large pending fact crosses target | Keep exact continuation and fixed target; next call advances rather than restarts. A real252MiB semantic history plus an unfinished physical prefix crosses the target, resumes at saved D, then finishes via its valid abort. `oversized_pinned_receiver_finishes_its_captured_pass`, `pending_fact_crosses_cache_target_and_resumes_without_replay` |
| Captured pass | Writer appends indefinitely | Finish finite captured head, then permit eviction/new pass. `independent_committed_read_uses_physical_facts_while_a_writer_exists` (captured514/observed515). |
| Captured raw tail | Semantic tail torn or typed refusal | Release pin; another receiver can be admitted. `oversized_pinned_receiver_finishes_its_captured_pass` and `failed_source_join_releases_the_partial_pass` |
| Full history beyond160 MiB | Checkpoint/restart | Stream bounded chunks and restore full state+A. `checkpoint_streams_history_beyond_one_fact_limit` |
| Checkpoint chunks | UTF8/escape splits, missing/reordered/tampered chunks | Existing JSON/token/state owners validate the chain; typed refusal before publication for malformed content. `checkpoint_chunk_boundaries_and_damage`, `checkpoint_stream_reader_handles_physical_utf8_and_escape_splits` |
| Pinned partial read | Source worker join fails after successful page progress | Preserve A/D and semantic snapshot, release fixed pass, mark freshness unknown; typed read failure. `failed_source_join_releases_the_partial_pass` |
| Newer observed head at same D | Older Current read delivered after newer Stale read | Validated observed-head fence refuses the older replacement; A/D alone cannot order freshness. `older_observed_head_cannot_reenable_committed_controls` |

## Browser verification evidence

`committed-transcript.mjs` passed 72/72 checks in Chromium and WebKit at
360, 600 and 1440 pixel widths. Each unconfirmed state produced one notice and
zero enabled permission controls; complete empty and absent live attachment
produced zero notices and controls. Complete with published authority produced
two enabled controls, including after the incomplete transitions. Horizontal
overflow and collected console/page/request errors were zero. This checks the
production components, the actual client decoder and applyView presentation;
the server tests own exact live execution authority. Three real question siblings
retain the middle radio selection when the conditional interaction display-limit
notice appears and disappears, with Stop guidance and no horizontal overflow. The fixture is served by Vite in development, without a
production performance claim.

The receiver checkpoint store must preserve the ordered chunk set atomically
with A. Codec validation does not authenticate an otherwise valid changed prompt
or detect removal of bytes that happens to produce a different valid semantic
state. The receiver fixture checks contiguous stored chunk indices before restore.
Scope/A/lifecycle validation is independent of this local persistence assumption.

## Exact cache identity and invalidation agreement

The key is the core owned `Scope` (already Eq/Hash). The core Id constructor
owns byte validation and compact retained identities before any clone. All historical
scope entries stay in the measured map while a read owns them. Runtime stream
lookup and per-page identity checks remain independent of cached scope; the cache
confers no authorization. The cache lifetime owner is one atomic idle/pinned/
obsolete state, so obsolescence cannot race a later pin back to active. A missing
stream or independently observed replacement incarnation retires affected entries
under the map lock without waiting for receiver guards. An old worker retains its
entry through actual source join, then checks obsolescence under the same map
owner before its publication point. A completion linearized before invalidation
may return its scoped view; one linearized after invalidation is refused. Consumers
still own their exact scope/monotone replacement fence. Retired inactive entries
can be evicted, and no safely evictable entry yields typed ReadCapacity. Count and
byte accounting includes the old scope key, entry and intentional history.

| State | Ordering | Decision / regression |
| --- | --- | --- |
| Old incarnation, read in progress | Replacement observed before old completion | Retain/account old entry until join, prevent re-pin, refuse obsolete publication. `replaced_and_missing_stream_keep_owned_entries_accounted` |
| Old incarnation, read in progress | Stream missing before old completion | Same owner retirement; missing-stream read returns None, old read refuses. Same regression. |
| Publication and invalidation both ready | Each linearization order | Map-owned final publication check orders the result without map-to-receiver locking. Same regression. |
| Old entries active at entry/byte target | New current scope admission | ReadCapacity until safe eviction; existing ownership remains measured. Same regression. |

## Current permission actionability agreement

Committed pending-review facts and live command authority have different owners.
The domain `ExecutionSession` private `ActiveExecution.pending_permissions` remains
the sole selection/cancellation owner. Its existing collection moves behind a
short shared lock and issues an immutable session/execution-scoped weak read
handle. That handle has no mutators or own pending state. Domain answer removes
the review before awaiting application audit; settlement/cancellation empties the
same collection, and dropping its sole owner makes weak reads unavailable.

`ExecutionController` publishes the domain-issued handle only after successful
begin-execution. An ACP carrier contains only the last issued weak handle; its
only publisher is this owner path, and it has no current/pending flags or lifecycle
decisions. The backend exposes synchronous optional acquisition; absence means no
current authority. It cannot queue behind an answer's audit or command queue.
The ProviderSession wrapper validates exact session identity, and the Agent's
existing lifecycle owns session/execution/work-generation matching before and
after the read. The Agent stores no current-review ledger. Stale carriers confer
nothing. Unexpected poison is typed; absence/drop is ordinary unavailability.

Gateway actionability intersects the committed pending fact with this exact live
membership. It never manufactures a persisted answer, outcome, receipt or
freshness. Restored/remote pending history without a live owner stays unresolved.
The test fixture uses the same domain/controller owner, rather than only claiming
Consumed in an error return.

| State | Ordering | Decision / regression |
| --- | --- | --- |
| Committed review and exact live owner | Answer gated before domain consumption | Review remains offered; identical AuditFailure leaves Pending when the owner still contains it. `current_permission_authority_tracks_consumption_before_audit` |
| Domain consumes review | Actual audit blocked/fails afterward | Immediate synchronous authority read suppresses a second answer; audit diagnostic does not determine membership. `agent_acp_permission_query_does_not_wait_behind_consumed_answer_audit` and the gateway regression. |
| Query and consumption concurrent | Both lock orders | Membership query linearizes under the existing collection lock; commands still revalidate their owner. Same regression. |
| Agent query captures a live context | Stop and provider replacement happen before backend acquisition returns | Existing lifecycle fence refuses the older context even if its custom backend retains a pending domain handle. `permission_query_refuses_a_generation_replaced_during_acquisition` |
| Active permission owner | Cancel/settle/drop | Invalidate only that execution; old handle cannot gain authority over a newer execution. `permission_authority_is_scoped_and_weak` |
| Committed pending review | Restart/remote absence | Preserve semantic fact and unresolved display; offer no controls. Existing committed renderer authority tests. |
| Custom backend | Absent, foreign or stale handle; poisoned owner | Explicit unavailable or typed refusal before effects; preserve transcript. `current_permission_authority_tracks_consumption_before_audit`, `permission_authority_reports_poison_and_drop_without_retaining_owner` |
| Incoming main assembly (#314/#311/#321) | Reasoning control keeps the existing scheduler idle check and exact provider generation; permission read membership remains synchronous through the existing lifecycle/carrier and never enters that command queue. Focus recovery and pane picture/home layout remain owned by their merged desktop adapters/components. | Union of imports and adjacent backend methods only; no new transcript fact, freshness or permission collection. Active permission refuses an effort command; effort replacement cannot confer old permission authority. Combined Agent/ACP permission authority and merged effort tests; full frontend plus real committed/focus/responsive browser checks preserve incoming behavior. |

## Automated review resource correction (PR #323)

The exact published SDK head `11508f5b` was reproduced through the public fold
and checkpoint APIs. Applying 128/256/512 valid inputs with 4 KiB prompts requested
51/199/787 MB of cumulative allocations in one batch. The prior fold deep-cloned
and rescanned the growing semantic snapshot at every terminal. A checkpoint with
100000 shaped queue events passed preflight and requested 56 MB during owned
restore before semantic rejection. These are defects; earlier clean local review
and green checks do not close them.

The accepted representation decision keeps the current public `SessionSnapshot`
contract. The existing records semantic owner will retain its canonical owned
state and the actual invocation index, history, queue, provider-evidence and
allocation continuation. A reversible transaction stages only touched fields,
appends and derived-owner changes; dropping it rolls back unless the caller
explicitly commits. The public receiver can keep this guard through its SQLite
checkpoint/A/effect commit, so SQL or audit failure does not require cloning the
old fold. The transition match remains in that semantic owner; full admission and
incremental decisions share the same validation helpers and domain authorities.

Full immutable public snapshot materialization at the actual read-publication
boundary is an explicit O(full retained history) cost, separately accounted from
bounded suffix work. It must not occur inside apply, checkpoint encoding or
retained-byte accounting. Checkpoint encoding borrows canonical state through the
existing codec. This choice does not claim that publishing an immutable complete
snapshot costs only the downloaded suffix. Persistent public collection APIs and
a new persistent-collection dependency are outside this decision.

The previous retention validator had a retroactive relationship:
it first finds later permission cancellations and then retains their earlier
request payloads during replay; other requests are released hypothetically. A
later cancellation can therefore change the earlier retention witness. Reusing a
single forward disposable controller without representing this retroactive
relationship would change accepted history. The correction stays at that
retention owner and preserve the controller's actual count/payload policies; no
mirrored live pending ledger or invented persisted answer is permitted.

| State | Ordering | Required decision / evidence |
| --- | --- | --- |
| Valid restored continuation | Repeated growing pages or 1 versus 64 complete facts | Actual semantic indices/history/queue/provider state is reused; no prefix cloning, replay or accounting scan per fact/chunk. Work/allocation counters measure suffix separately from full publication. |
| Valid terminals followed by malformed final frame or decision | Transaction has staged appends and changed small fields | Roll back the whole batch, including A/D/fact count/status, correlations and resource accounting; prior checkpoint and held published snapshot remain unchanged. |
| Active guard replaces a failed acknowledgement | External commit succeeds; moved old typed diagnostic is released | Commit adds no validation or typed refusal. Existing value destructors can allocate bounded teardown work for retained diagnostic trees; this is separate from semantic staging and full publication. Actual replaced-ack commit and checkpoint restore retain typed read/runtime causes; guard Drop restores the prior failure. |
| Receiver transaction accepted by SDK | SQL checkpoint/A/effects commit fails or waiter drops | Borrowed guard rolls back unless commit is confirmed; no cloned receiver candidate or second semantic fold. |
| Canonical state advances | Prior immutable published snapshot is still retained | Publish a separately owned complete snapshot only at the read boundary; old snapshot remains immutable and both allocations are measured. |
| Exact receiver has unchanged semantic state | Another read publication is requested | Return equal full semantic values in a separately materialized immutable result; Arc pointer reuse is not promised. Prior results remain unchanged after subsequent physical source updates. Each publication explicitly costs O(full retained history). |
| Pending physical pieces | Partial, seal, abort or invalid suffix | Existing FrameValidator remains sole physical owner; Arc pieces are staged without copying a prefix; body decodes once at seal. |
| Sparse tool state | Accepted replacement followed by rollback | Move untouched large payloads into the replacement and touched old fields into entity-issued undo; rollback reconstructs the old value by replacement, without cloning the old tool payload. Pointer-allocation tests cover omitted and cleared fields. |
| Queue state | Admit, select, reorder, restore, then reverse undo | Restore exact membership/order and remove only newly admitted seen identities. Older dispatched identities remain unavailable for reuse; the queue stays non-Cloneable. |
| Queue, provider context or steering target | Contradiction occurs across two complete facts in one batch | Each logical boundary must be valid, even if a later fact would hide the contradiction; sole semantic transition helpers refuse atomically. |
| Two individually valid reviews larger than half the controller review budget | Both requests arrive; cancellation of the later request is feasible; subsequent cancellation of the earlier request proves retroactive overlap | Refuse the second cancellation atomically at the existing retention owner. If the earlier request has no later cancellation, its hypothetical answer witness remains valid; cancelling earlier before admitting later also permits the valid nonoverlapping counterpart. No answer/pending fact is invented. |
| Untrusted checkpoint queue array | Queue history precedes or follows invocations | Shape preflight consumes the semantic owner's published structural maximum, never a copied 4096/multiplier. Refuse excessive elements before owned QueueEvent allocation. Exact invocation-relative semantic validation remains authoritative. |
| Queue checkpoint at the structural limit | Immediate successor exceeds limit | Positive valid history and negative excess, both JSON field orders; allocator/preflight and owner mutation evidence distinguish refusal before allocation from eventual semantic rejection. |

The historical-retention correction consumes controller-issued
`HistoricalReviewCost` (the existing input/options/identity allocation charge)
and `ExecutionController::validate_review_totals`. Live review admission consumes
that same aggregate decision. A historical request point includes its own
admission charge before hypothetical release; a later cancellation adds its
charge to subsequent request points up to cancellation, then queries maxima
through the controller decision. Cancelling the later of two large reviews first
is feasible; subsequently cancelling the earlier review makes the later request
point infeasible. Cancelling the earlier review before the later request arrives
is the nonoverlapping valid counterpart. Sparse tool merging, tool count/bytes,
seen permission identities and individual payload checks remain owned by the
forward disposable controller, independently of the derived interval summary.

Queue replay retains the existing non-cloneable `InvocationQueue` entity. Its
reversible mutation token moves selected/removed identities and replacement
pending deques, and removes only a newly admitted seen identity on rollback.
Reorder validation and reconstruction remain in the existing order owner used
by both public order application and semantic replay. Pending work is bounded
by the existing queue capacity; the entire seen-ID prefix is never cloned or
replayed to stage a decision. Token allocation and retained seen-ID accounting
belong to that entity. The physical guard also stages head/empty observations,
so external receiver commit failure can restore their exact metadata.

Allocation evidence distinguishes retained payload copies from amortized slot
growth in the existing `Vec`/`HashMap` owners. MessageChunk's test-only clone
counter measures actual copied text bytes; full validation counters measure cold
prefix replay. Growing-page tests must copy only new message payloads and perform
no cold prefix validation. Collection capacity can grow or remain after rollback;
the retained owner charges those actual slots. The standalone allocator fixture
measures all requested allocations separately from full read-publication copies.

Derived allocation helpers charge separately allocated map/vector slots, compact
identity text and Arc control/payload nodes. Inline owner layouts are charged by
the enclosing continuation or invocation-vector slot, rather than repeated by
every helper. Disposable historical-controller pending-map spare capacity and
its issued weak-handle carrier remain owned allocations after hypothetical review
release; accounting includes them without inferring any live pending decision.

Checkpoint Event serialization uses the single borrowed records mapping; its
Text/Thought fields borrow the original chunk. The owned decoder keeps String
fields and the wire bytes stay identical. Checkpoint traversal/output allocation
is O(full history), distinct from suffix application. Existing nested field
codecs may construct one validated bounded Metadata/Event DTO at a time; those
working values obey the existing message, tool/review, question and error owners,
while the ChunkWriter retains the intentional full encoded checkpoint. There is
no simultaneous full snapshot DTO clone. Byte equality and roundtrip fixtures
exercise the shared owned shape, and a growing output fixture requires no old
message payload clones during checkpoint traversal. The quota-limited ChunkWriter
seam is separately owned by #261 and will consume the same borrowed mapping.

The borrowed semantic codec keeps the provider-report DTO in a private Box to
reduce the generic enum's fixed slots. Serde encodes the same report fields; this
is working representation only, with no new transition or admission decision.
The derived interval recursion carries its request range as one pair; its tree
and controller-issued decisions are unchanged.

A live borrowed guard owns suffix undo until commit/drop. Undo retains moved
replaced field payloads, changed domain metadata and per-change interval nodes;
its size follows the staged suffix and the existing touched component limits,
rather than a copied prefix. `retained_bytes` describes canonical continuation
allocations and retained spare slots, while this temporary undo and the incoming
physical/decoded suffix are separate working allocations. The storage reader's
existing fixed frame/page admission bounds its staged batch. A public receiver
chooses its own input batch size; the guard does not claim constant memory for an
arbitrarily large caller-provided batch. Checkpoint output and explicit published
full snapshots are intentional separate O(history) allocations.

The truthful commit contract separates refusal from value release. On an active
guard, commit performs no new validation or source observation and cannot return
a new refusal; prior rejected staging is the only disabled-guard error. Existing
`StorageShutdownFailure::drop` uses an iterative Vec worklist under the shared
diagnostic tree bounds. A standalone allocator fixture measured actual replaced
acknowledgement commit teardown for 3/31/127-node trees: 64/1344/4416 requested
bytes and 1/17/65 allocation calls. This disproves a no-allocation comment but
does not show a resource/lifecycle bound violation. The existing destructor and
typed error codec are preserved; no intrusive representation is introduced.

## Single materialization after source completion

| Receiver state | Actual ordering | Owned result and cost |
|---|---|---|
| Valid bounded page/catchup | Before source.finish joins the physical worker | Keep only typed Result<()> success, canonical continuation and physical/head evidence. Do not construct a full public CommittedSession. |
| Successful source join | Final exact-key bounds/head observation succeeds | Observe physical tail through the existing fold owner, then construct one immutable full CommittedSession for the existing map-owned publication fence. Repeated warm unchanged public reads each materialize once; prior published snapshots remain immutable. |
| Valid suffix, source join fails | Join refusal after semantic/physical catchup | Existing finish_owned_read marks unknown and releases the captured pass pin while retaining A/D and semantic state. No full snapshot is constructed before this failure. |
| Source joins, postjoin source reset/deletion or metadata failure | Actual bounds fails before final publication | Existing typed error, exact-key retirement where proven, abandoned pass and prior evidence remain; no discarded full snapshot. Generic failure cannot authorize retirement. |
| Source joins, physical tail changes | Sealed fact or unfinished suffix arrives before bounds | Preserve captured pass and A; observe newer physical head through the existing freshness owner, publish once with stale evidence, then eventual reads advance to current independently of semantic completeness. |

Full result publication remains intentional O(history) work. The prejoin typed
completion is not a new source authority or projection. Tests instrument the
actual semantic snapshot_handle materialization per retained transcript and
exercise RecordStorage::read_committed, including repeated warm reads and
postjoin interleavings. The source join and final metadata/publication fences
remain unchanged.
