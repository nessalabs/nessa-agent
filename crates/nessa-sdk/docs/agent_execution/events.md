# Committed semantic events

`SessionStorage::events` opens an independent observer. RecordStorage implements
it from saved semantic decisions; InMemoryStorage and custom adapters using the
default port return `EventReadError::Unsupported`. Snapshot replacement views do
not retain all intermediate decisions and are not used to synthesize events.

The host authorizes the conversation before opening or reading. The reader owns
no writer lease or provider control. Reading does not start, resume or stop an
agent. Dropping the reader releases its observation registration after any
admitted physical read finishes. Dropping a read future abandons that answer;
discard that reader and reopen from the last cursor the consumer received.
Shutdown closes new reads and drains admitted physical work through the shared
record-storage owner.

Use `EventStart::Beginning` for retained history, `Now` for the complete save
captured at initial read, or `After(cursor)` for exclusive resume. Cursors identify
the session, actual storage incarnation, terminal unit location and original
change index. `encode`/`decode` own the opaque bounded transport representation;
the token grants no access. Decode validates syntax and session, and replay
validates the exact location. Erase/reset returns `IdentityChanged`; unavailable
retained history returns `HistoryPruned`. An invented boundary/index returns
`InvalidCursor`, including a cursor ahead of committed publication.

Call `reader.read(EventReadLimits::new(count, bytes)?)` cooperatively. `Preparing`
means another bounded step is needed. `Page` exposes immutable event envelopes
and their original ordered `SessionChange` through borrowed inspection; `Gap`
means the transcript fold rejected a saved record. An unknown physical schema
that cannot be translated into a fold input instead returns typed
`Storage(Corrupt)`; it is not presented as a decoded semantic gap. Gaps beyond the transcript
UI's 32-line summary remain observable. The envelope cursor is also its stable
identity across restart and replay. `Idle` supplies a committed boundary cursor;
it is not a promise that another process will publish a notice.

Register `RecordStorage::watch_committed` before the final source recheck to
avoid a local lost wake. Its notices coalesce and carry no events; reread the
source and retain a fallback timer for changes through another process. The
reader itself never waits for a future commit or invokes a consumer callback.

Certification scans a fixed physical ceiling in at most 16 frames and 1 MiB
returned accounted physical bytes per step. Emission advances at most one frame
per step through the certified outer-save publication. A sealed unit before
`SaveComplete` remains hidden. Extraction uses the transcript's existing atomic
semantic validation and rollback, including its rules for skipping dependent
changes after a gap. It preserves transitions such as context A to B to A in one
save, even when the final snapshot is unchanged.

The adapter retains any number of readers per store; each belongs to a follower its host bounds. At most four physical read steps run at once, and a step past that is refused `Capacity` for the caller to retry. Output pages accept at most
256 events and 256 MiB encoded semantic payload bytes. The pending output holds
at most one decoded unit, whose encoded body is limited by the existing 160 MiB
fact bound. Each single change's encoded semantic batch is charged; gap payload
is one byte. Envelope allocations and cursor tokens are additional bounded
per-event overhead. `ItemTooLarge` leaves the next event pending so a caller can
raise its page allowance; no event is silently skipped. For consumers that map
large saved tool/text observations to small metadata, use the full 256 MiB
source budget and enforce the smaller output budget after projection.

These are work/page bounds, not constant total process memory: two folds retain
conversation history, pending physical chunks can coexist with assembled and
decoded bytes, and a 160 MiB encoded body can occupy more memory. The additional
scan and retained fold avoid collecting an unbounded outer save in an observer
queue. Slow readers replay history and do not block provider progress.

| Ordering | Contract and regression |
| --- | --- |
| A to B to A in one save | All transitions survive, original indices and order; `replay_preserves_transitions_inside_one_save_and_original_indices_on_resume` |
| Replay/restart and resume midway | Same IDs, no skipped suffix; same test |
| Partial fact, sealed unit, outer completion | No event before SaveComplete; `partial_fact_and_sealed_unit_stay_hidden_until_outer_completion` |
| More than 32 unreadable units | Later gaps still appear; `all_separated_invalid_units_emit_gaps_after_summary_limit` |
| Small page then larger retry | Typed refusal preserves pending event; `item_budget_refusal_preserves_pending_event_and_reset_invalidates_cursor` |
| Reset after read / old resume | Typed IdentityChanged; same test |
| Invalid index, many retained readers, shutdown | Typed refusal and no reader cap; `invalid_cursor_is_typed_and_retained_readers_are_not_capped` |
| Cancel read, then drop handle while worker waits | Registration remains occupied until owned worker completes; `cancelled_read_and_dropped_handle_retain_registration_until_worker_finishes` |
| Final metadata check fails after decoding | Pending payload and delivery cursor survive for retry; `failed_final_metadata_check_preserves_pending_payload_and_delivery_cursor` |
| Custom page with mixed identities, wrong next/head, empty/oversized or unordered events | Constructor rejects before publication; `page_constructor_rejects_contradictory_adapter_output` and `page_constructor_accepts_ordered_correlated_changes` |
| Snapshot adapter or malformed cursor token | Explicit unsupported/invalid outcomes; `unsupported_adapter_and_tokens_fail_without_provider_or_history_side_effects` |

The observer API publishes saved evidence, not provider effect acknowledgements.
A stop decision does not prove cleanup, receipt updates do not replace provider
settlement, and a later local failure does not erase earlier saved outcomes.
Those meanings remain with the existing session/agent lifecycle authority.

Custom storage adapters can construct immutable `CommittedEvent` and
`CommittedEventPage` values through their documented constructors. They own
history validation, captured publication evidence and requested byte budgets.
The fallible page constructor owns structural correlation: nonempty bounded
event count, one session/incarnation, strictly ordered event locations, next equal
to the last event, and an enclosing boundary head. DTO construction performs no
source read or policy decision. These adapter obligations are part of the reader port;
a host must still authorize the requested source before consuming it.
