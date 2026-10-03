# Bounded terminal discovery

The architectural decision is owned by
[ADR 315](../adr/done/315-bounded-terminal-discovery.md); this document holds
its bounds, ordering and enforcing tests.

Issue: [#315](https://github.com/nessalabs/nessa-agent/issues/315), prerequisite for
[authorized record reads](https://github.com/nessalabs/nessa-agent/issues/296) and transcript folding.
The request owner is published by [sync-engine #31](https://github.com/nessalabs/sync-engine/issues/31); identifier construction and its bounds are published by [#30](https://github.com/nessalabs/sync-engine/issues/30). The final dependency pin `8ce6a8226913cf8c455cab5a2f4e23ebadf01fb2`
also consumes the SQLite alignment in [#34](https://github.com/nessalabs/sync-engine/issues/34).
A simple external Rust client can therefore compose SDK record storage with the
core SQLite receiver under one native SQLite dependency; native app shells are
outside this prerequisite.

The prerequisite SDK identity surface moves with this change: `record_identity`
reads only stream metadata, `RecordStreamIdentity` carries its exact incarnation,
and `record_source_expected` rechecks it before constructing a worker. This lets
an authorized host compare scope before spending worker capacity, and prevents
a reset between lookup and construction from checking out progress for the wrong
stream. `MAX_PHYSICAL_RECORD_PAYLOAD_BYTES` publishes the tagged frame bound
owned by the framing codec. Product schema/client generation remains #296.

The generic sync `RecordSource::head` contract still returns a fully validated
committed head. Product callers use `NessaRecordSource::bounded_head` and
`bounded_page`: each discovery step validates at most sixteen frames with at most one
MiB of returned event-store accounted record bytes against a captured physical tail.
The pinned SQLite reader decodes at most sixteen candidate records, including
one byte-limit lookahead. Returned validation bytes are at most
`MAX_STORED_RECORD_BYTES` (one MiB); decoded accounted bytes are at most twice
that cap (two MiB), because each stored record is independently capped by the
same published constant used by runtime configuration. Test counters measure
returned records and bytes only, never all decoded work or physical disk I/O. `RecordReadStatus::Preparing` means another
bounded step is needed; it exposes no physical or committed head.

`stream_fact::FrameValidator` owns start, piece, seal and abort validation.
`decode_first` and `validate_abort_prefix` use it too. It retains only a start
header and SHA-256 state, with no semantic body. Body consumers receive validated
piece slices and choose their own retained storage.

`RecordStorage` owns a sixteen-entry process cache keyed by the exact stream key
and incarnation. A cache entry contains monotonic forward framing/group progress,
a fixed captured ceiling, a64-entry completion-proof ring, and at most one
separately allocated historical framing/group pass. An operation checks out that state
before I/O, leaving an occupied entry. Its drop guard returns progress on normal
completion or unwind. Occupied entries cannot be evicted or replaced by a second
owner; capacity exhaustion returns Preparing. Cache eviction and process restart
may repeat bounded validation. The shared core `validate_page_request` consumes source-owned capacity limits
before metadata I/O in either page path. Scope correlation and physical
incarnation/floor/tail/terminal checks are distinct SDK source relationships.
Invalid scope or page budgets do not check out cache state. A malformed stored frame refuses
without advancing its offset/hash; valid earlier frames in that step may retain
their validated prefix. The cache contains no source worker. Source drop
and physical operation ownership follow the existing SDK source contract.

Successful GroupProgress Complete validation supplies the exact publication
proof. Unit seals do not. Retained proof reuses that publication after current
physical bounds are checked under checkout; an evicted proof requires a historical
scan through its fixed query. A ready page then uses the existing sync page
count/payload limits. That read returns at most one runtime record-cap of
accounted bytes and may decode one additional capped lookahead. Discovery and
page retrieval are separate finite reads; the two-MiB ceiling above describes
discovery, not their sum. A ready `bounded_page` call decodes at most four runtime
record caps of accounted bytes (two discovery and two page).
Stream replacement and pruning remain typed refusals from the event runtime.

## Shared discovery correction orderings

PR401's correction has three root-verified original-production failures; the
corrected candidate still requires final checks. The cache owner observes physical incarnation, floor and tail
under its exclusive checkout. A captured head ceiling and an exact requested
publication are separate queries; neither supplies physical-tail evidence.
The same owner retains monotonic forward framing/group progress, one bounded
completion-proof ring, and at most one historical pass. Older queries do not
reset forward progress, its captured ceiling, or its known later failure.

| Row | State and ordering | Required result and regression boundary |
| --- | --- | --- |
| D1 | One actual source proves head32; a fresh actual source pages completed20, then Unit19 | Page20 succeeds under the unchanged physical stream; Unit19 refuses InvalidRequest; head32 remains available. `fresh_reader_pages_older_publication_after_another_reader_proves_newer_head` uses real lease saves and two public sources. |
| D2 | A completed20 proof survives newer head32; another source requests several bounded pages20 | Ready20 reuses the shared proof with zero additional returned discovery frames; head32 remains warm. `proven_historical_pages_remain_ready_after_newer_head` checks public Ready pages and preserved newer head. Exact zero-extra-discovery accounting remains OPEN. |
| D3 | Forward head192 is proven and older40 proof has been evicted; historical40 starts, then Unit11 arrives while it is active | The competing call boundedly advances original40 and returns Preparing. Original40 finishes before11 acquires a separate scan and refuses InvalidRequest; head192 retains its proof. `competing_historical_query_advances_original_scan_and_preserves_forward_head` checks public Preparing/Ready progression, original completion and competing Unit refusal. Exact16/16/8/11 counts and zero forward replay remain historical/private observations, not current public acceptance. |
| D4 | Head captures X; another source validates or retains Xprime>X before the first obtains the cache owner | Read current physical bounds after checkout, preserve the first captured X and return the last completed publication at or below X. Deterministic public scheduling evidence is still required; the concrete SQLite runtime has no existing public pause between capture and checkout. |
| D5 | A newer physical suffix is corrupt; an earlier known or evicted publication is requested, then head is retried | Earlier permitted proof/read may succeed without clearing the known forward failure; repeated head refuses Unavailable with zero extra discovery reads. `historical_miss_preserves_known_forward_failure_and_clean_head` corrupts only a newly appended physical row after public saves and includes the clean counterpart. It observes the sticky typed result; exact no-extra-read accounting remains OPEN. |
| D6 | The source is dropped between bounded historical responses; an idle entry is evicted or cache/process restarts | Checkout/drop ownership returns actual progress. `historical_scan_survives_public_source_drop_and_recreation` drops the first actual source after a Preparing response and a replacement reaches the same historical publication and newer head. Exact16/16/8 returned work is historical/private evidence, not current public acceptance. In-flight historical answer cancellation has no existing public cancellation API and remains unverified. Existing abandoned-answer, eviction, occupied-owner and restarted-process fixtures cover their named boundaries. |
| D7 | A retained proof is followed by actual Reset, prune, or same-incarnation shrink | Current physical evidence refuses IdentityChanged/Pruned before reusing proof. `shared_completion_proof_refuses_reset_and_same_incarnation_shrink` uses public lease.erase and an actual SQLite tail shrink; a new Reset incarnation validates independently. True policy-prune acceptance through a supported public SDK seam remains OPEN. |
| D8 | An active historical query finishes while a different unproven query polls; alternatively a prior validated Complete is requested | Finish the original finite scan, retain its exact query/result until a new unknown query acquires, and return Preparing to the different nonproof query. A known Complete can answer immediately while the scan is active; Unit11 does not become such a proof. Stream replacement/pruning errors during the original physical read apply to both queries and propagate. D3 and `historical_scan_survives_public_source_drop_and_recreation` exercise completion/proof paths; deterministic replacement/prune during that read remains unverified. |

The completion-proof retention moves from the source's existing64-entry policy
to this one cache owner. A historical miss costs O(T) total returned validation
frames over bounded calls. Repeated eviction/churn can repeat that cost; no global
zero-replay or O(H) full-session claim follows. Counts exclude SQLite decoding,
lookahead, disk I/O, allocator overhead and whole-process memory. Exact private proof-ring capacity and inline/dynamic cache allocation measurements
are withdrawn as current public acceptance evidence. Earlier frozen-source
metadata logs remain historical; the competing-query regression retains public
Preparing/Ready/InvalidRequest outcomes. Exact private read counts, cache release and heap accounting remain OPEN.
No public test accessor or private state-layout assertion substitutes for them.
The drop guard moves original allocations back into
the idle cache instead of cloning both scans. A cache slot's inline storage and
an active Owner's inline storage coexist during checkout; the transferred dynamic
proof and framing/group allocations have one owner. Key text, cache spare
capacity and an active owner's key copy are separate retained costs.

| Ordering | Transition | Enforcing test |
| --- | --- | --- |
| Invalid request followed by stream reset | Core admissibility refuses before SDK stream metadata can report replacement; generic and bounded page agree and retain no progress | `invalid_page_request_refuses_before_replaced_stream_metadata` |
| Valid body assembled from many pieces | Allocate exactly the validated declared body length before assembly; accepted body retains no spare growth capacity | `decoded_body_retains_exact_declared_capacity` |
| Many small inline facts, then unchanged head | Each step <=16 frames even when the byte budget could admit more; repeated unchanged head reads no historical frames | `long_inline_history_enforces_frame_step_limit_and_unchanged_head_does_no_read_work` |
| Fact exceeds one step; source recreated after each response | Preparing until captured tail is validated; each step <=16 frames, total work equals history length | `recreated_sources_resume_bounded_large_fact_validation_and_pages_do_not_rescan` |
| Writer appends while discovery is pending | Finish old captured tail before capturing the later tail | `discovery_finishes_captured_tail_under_new_writes_then_discovers_later_head` |
| Partial fact reaches captured tail, then seal arrives | Return last terminal H; preserve hash/header to validate the later seal | `partial_tail_preserves_hash_until_seal_and_unknown_target_stays_invalid` |
| Unknown target lies inside validated partial fact | Reject nonterminal target; retain progress | `partial_tail_preserves_hash_until_seal_and_unknown_target_stays_invalid` |
| Answer consumer disappears before physical completion | Source final drop joins operation; progress survives abandoned answer | `abandoned_answer_retains_progress_and_cold_cache_repeats_only_bounded_steps` |
| Cache evicts idle state or restarts cold | Repeat bounded steps, then reach same committed H | `abandoned_answer_retains_progress_and_cold_cache_repeats_only_bounded_steps` |
| Competing operation for an occupied key, full cache, panic | One owner per key; evict idle entries only; return ownership on unwind | `cache_bounds_entries_and_exclusive_owner_returns_progress_after_unwind` |
| Occupied stream or sixteen occupied cache entries | Preparing without physical validation or replacement owner | `occupied_stream_and_full_active_cache_refuse_without_replacement_work` |
| Invalid scope or page limits | Refuse before cache checkout, preserve empty progress | `invalid_scope_and_page_input_do_not_check_out_or_publish_progress` |
| Warm cache followed by actual reset or retention-floor advancement | Old source refuses identity change or pruning; new incarnation starts its own validation | `cached_old_incarnation_and_pruned_prefix_are_typed_refusals` |
| Partial cached prefix then abort, later fact, malformed tail | Abort and later fact advance terminal evidence; corrupt frame keeps prior valid offset | `cached_partial_prefix_accepts_abort_then_later_fact_and_refuses_corrupt_tail` |
| Byte-limited valid/malformed lookahead | Byte-limited step retains only bounded returned prefix; next step validates or refuses lookahead without advancing corruption | `byte_limited_lookahead_returns_only_prefix_then_validates_or_refuses` |
| Maximum accounted invalid first frame | Refuse typed corruption within one MiB returned validation budget and publish no progress | `maximum_accounted_corrupt_record_is_bounded_and_publishes_no_progress` |
| New OS process opens durable SQLite records | Cold bounded rescan returns the same committed terminal | `restarted_process_revalidates_durable_stream_in_bounded_steps` |
| Changed pieces, malformed header/inline/seal/abort, and rejected frame retry | Refuse typed framing corruption without changing the failed offset/hash; original suffix still succeeds | `incremental_validator_refuses_header_inline_piece_seal_and_abort_without_changing_state` and `stream_fact::tests` |

```mermaid
sequenceDiagram
    participant P as Product read thread
    participant C as SDK progress cache
    participant S as Physical record store
    P->>C: Check out exact stream state
    C-->>P: Fixed tail, offset, frame hash/header
    P->>S: Read <=16 candidates / <=2 MiB decoded
    S-->>P: Immutable frames through captured tail
    P->>P: FrameValidator advances terminal evidence
    P->>C: Return progress; release exclusive owner
    P-->>P: Preparing, or Ready(committed H)
    Note over C: No worker or semantic body retained
```

The discovery work bounds cover candidate records decoded and returned frames hashed
per step. They do not promise
interruptible SQLite I/O or a wall-clock completion bound for a stalled disk.
An abandoned caller does not cancel physical work. The host must retain its read
permit and runtime through source worker completion, as specified by #296.
