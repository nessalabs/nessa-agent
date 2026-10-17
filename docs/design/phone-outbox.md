# Phone outbox

Issue: [#269](https://github.com/nessalabs/nessa-agent/issues/269).
The send id, the receipt lookup, the five rendered outcomes, and replay are
the [phone send contract](https://github.com/nessalabs/nessa-agent/blob/b2d6a2d86d1bbe09107c123df70a6bf74960adf3/docs/design/phone-send-contract.md)
(pull request 885). This page is the device ordering around that contract.
The contract is not restated here and is not edited by this work.

The outbox lives in `nessa-client-core`. Cedar admits the session. This
outbox does not decide who may send.

## At least once

The client guarantee is at least once, not exactly once. One command id is
minted when the draft is held and is never replaced because a save was lost
or two callers raced. The server dedupes `conversation.send` on that id: the
same bytes return the original receipt and do not enqueue a second turn
(the contract's Replay section). The client may send those bytes again.

Exactly once on the device was rejected. The client cannot see the server's
commit while it is inside `conversation.send`. Four review rounds tried to
close that gap with compare-and-swap, outcome ranks, and a lease that covered
only retry. Each round found another interleaving, and the loser was reported
as `Conflict` (different bytes) or `Unavailable` (nothing was sent). Both tell
the caller to mint a new id and duplicate the message. The server already
dedupes. Exactly once belongs there.

Holding one SQLite transaction from departure through the network was
rejected. A crash would roll the departure back, and the next open would show
a draft for a send that may have left. Departure is its own commit, before
the lease.

After departure, a lost or conflicting save reloads the row and reports that
outcome. Unknown means maybe sent. It does not mean not sent. `Unavailable`
and `Uncertain` are returned only when departure itself did not commit, so
`conversation.send` was not asked. `Conflict` is returned only when the same
command id stores different bytes. Same bytes are never `Conflict`.

## States

Unknown does not claim submission. It does mean the send may have left.

| State | What it means | Same id sent again |
| --- | --- | --- |
| Draft | Saved. Not departed | The first send |
| Unknown, not admitted | Departed. No receipt | Yes, after a lookup found nothing, under the lease |
| Unknown, lookup failed | The lookup did not finish | Yes, same rule |
| Unknown, attempted | The server has an attempted receipt | No. A later lookup may adopt a settled receipt |
| Unknown, unresolved | `submission_unresolved` | No |
| Accepted, running, done | A queued, injected, or settled dispatched receipt | No |
| Failed | A typed refusal, including forbidden and unauthorized | No |

`turn_running` is failed on the original id. Its text is a new draft under
new ids. The original id is not sent again.

## One lease, and the transitions

The writer lease is the SQLite file lock. A draft send or an explicit retry
that takes the lease waits while another command holds it
(`a_second_command_waits_for_the_file_lease`). The first send takes it, and
so does an explicit retry. Departure commits first. The caller that committed
it reloads inside the lease and sends only when that committed row is still
unknown and still inside the foreground budget
(`the_departure_winner_reloads_before_it_sends`). The lease is then
held across `conversation.send` and the save of that answer, including a
`turn_running` save and its new draft, in that same commit. A second caller
that also loaded Draft, or an explicit retry whose lookup finds no receipt,
waits and reloads. It does not call `conversation.send` when a direct
answer is already stored, or when the foreground budget is already spent. If
the wait ends without the lease, it reloads and returns that row. It does not
return `Unavailable`, `Uncertain`, or `Conflict`. Unknown still means maybe
sent. A send whose answer is lost may already have left the device. That is
at least once. The server dedupes it. A send first reading a departed row only
reconciles the receipt. If that lookup leaves the row unchanged, it can return
unknown while another send is pending
(`a_sender_first_reading_departure_can_return_before_the_send`). Recovery does
not send and does not take the lease. Its write waits if a send holds the lease,
then precedence applies.

The lease commit is one transaction. If that commit is uncertain, the
transaction is rolled back and every transition it contained is written
again, in the same order. Each answer is the compare-and-swap that saved it.
A `turn_running` mint stays one write: the lookup, the return, and the new
draft. Replaying only the last answer leaves a history restoration rejects
(`an_uncertain_lease_rewrites_every_transition`).
If that mint fails after the send or the lookup has already returned
`turn_running`, the refusal is committed before the id error is returned
when that commit lands. There is no draft. A direct observation records
the send cause (`SendAnswered`, `ForegroundSend`, or `ExplicitRetry`). A
replay of an uncertain lease keeps that same cause: a `ForegroundSend` or
`ExplicitRetry` result is not rewritten as `SendAnswered`. A lookup records
`Lookup`. `ReturnedToDraft` names the draft, so it is not written when the
draft was not minted
(`a_turn_running_refusal_is_saved_when_the_draft_id_fails`,
`a_lookup_turn_running_is_saved_when_the_draft_id_fails`,
`a_replayed_foreground_send_keeps_its_cause_when_the_draft_id_fails`,
`a_replayed_turn_running_keeps_its_cause_when_a_weaker_answer_landed_first`,
`a_replayed_explicit_retry_keeps_its_cause_when_the_draft_id_fails`).
Recovery holds no lease, so its refusal is already its own commit and the
caller hears `Id`
(`a_recovery_turn_running_is_saved_when_the_draft_id_fails`).
A send already holds the lease, and the refusal commits with it. `Unavailable`
from that commit means the row was not written. The caller hears `Unsaved`,
and a later send asks the gateway
(`a_failed_lease_commit_after_turn_running_is_unsaved`).

`hold`, `send`, and `retry` record the person. `recover` records recovery.
A history reaches the row only by folding this table, so a history that
skips departure is corrupt. Restoration also rejects `Initiator::Recovery`
on `Departed`, `SendAnswered`, `SendLost`, `Unreadable`, `ForegroundSend`,
`ExplicitRetry`, and `Bound`. Recovery may record `Lookup`,
`ReturnedToDraft`, and `Held` on the successor draft it mints: that hold is
recovery creating a draft, not a person typing, and recovery does not send.
The row is not repaired
(`a_done_row_whose_history_stops_at_departure_is_corrupt`).
A lookup that returns `turn_running` records `Lookup` and then
`ReturnedToDraft` in that same commit, so restoration can tell that refusal
from a direct send. A direct `turn_running` records only `ReturnedToDraft`.
Departure's unknown is not a direct answer.

| From | Event | To | Who | Lease |
| --- | --- | --- | --- | --- |
| — | hold | Draft | Person, cause Held | No |
| Draft | hold, same bytes | Draft, the same row | Person | No |
| Draft | hold, different bytes | Refused. `Conflict`. Nothing is sent | Person | No |
| Draft | depart | Unknown, not admitted | Person, cause Departed | No. This commit finishes before the send |
| Unknown, not admitted | direct send answer | The answer's state | Person, cause SendAnswered | Yes, the first send, already held |
| Unknown, not admitted | send answer lost, lookup finds nothing, budget remains | Stays unknown, not admitted, then the same bytes are sent again | Person, cause SendLost, then ForegroundSend with the later answer | Yes, the same hold |
| Unknown, not admitted | that budget is spent | Stays unknown, not admitted | Person, cause Bound. Not Lookup | Yes |
| Unknown, lookup failed | that budget is already spent | Stays lookup failed. No send and no Bound event | Person, the waiter | Yes. The waiter holds the lease and writes nothing |
| Unknown, lookup failed | a later foreground call loses the send, then the lookup finds nothing, and the budget remains | Becomes not admitted, then the same bytes are sent again | Person, cause SendLost, then Lookup, then ForegroundSend | Yes, the same hold |
| Unknown, not admitted or lookup failed | explicit retry after lookup found nothing | Not admitted, the send count returns to zero, then the same bytes are sent again | Person, cause Lookup, then ExplicitRetry | Yes. The waiter reloads |
| Unknown, attempted | lookup finds a settled submit receipt | Done or failed, per the receipt | Person or Recovery, cause Lookup | No |
| Unknown, attempted | lookup lost, broken, not found, or not a submit receipt | Stays attempted | Person or Recovery | No |
| Unknown, lookup failed | lookup lost or broken | Stays lookup failed | Person or Recovery | No |
| Unknown, lookup failed | recovery finds nothing | Not admitted. No send | Recovery, cause Lookup | No |
| Any unknown | lookup is not a submit receipt | The row unchanged | Person or Recovery | No |
| Any unknown | lookup returns `submission_unresolved` | Unknown, unresolved | Person or Recovery, cause Lookup | No |
| Accepted, running, done, failed, unresolved | retry or recover | The row unchanged. No send | Person or Recovery | No |
| Failed, `turn_running` | — | That failed row, plus a new Draft, one commit | The send path: Person, inside the lease already held. Recovery: Recovery on both ReturnedToDraft and the new Held | Send path: the lease already held. Recovery: no |
| Failed, `turn_running`, draft id not minted | — | That failed row. No draft. The caller hears `Id`. A later send does not ask the gateway | Person, the send cause (`SendAnswered`, `ForegroundSend`, or `ExplicitRetry`) or Lookup. A replay keeps that cause. Not ReturnedToDraft | Recovery: no lease, and the refusal is already committed. Send: the lease already held is committed with the refusal |
| Failed, `turn_running`, draft id not minted, open lease commit unavailable | — | The departed row stays. The refusal is not written. The caller hears `Unsaved`. A later send asks the gateway | Person | Yes. `Unavailable` is a failed commit of the lease this send opened |

A direct answer is `conversation.send` returning queued, injected, settled,
a typed refusal, or `submission_unresolved`. Lost and unreadable are not
direct answers. They look the receipt up. An unreadable answer is not sent
again in that call.

A lookup does not produce running. Running is only the direct injected
receipt. Restoration rejects a `Lookup` event whose outcome is running
(`every_transition_and_precedence_pair`).

## Concurrent caller fixture orderings (#955)

The initial load selects two different existing paths. A caller that reads a
Draft contends for departure and then the lease. A caller first reading an
already-departed row reconciles its receipt without taking that lease. An absent
receipt can therefore return Unknown while another caller is inside the gateway.
The old S12 fixture did not select between these paths before asserting that both
calls remained blocked. Synchronize initial draft reads to test S12; retain the
late-reader ordering separately rather than requiring it to wait.

| Initial reads and ordering | Expected evidence | Regression |
| --- | --- | --- |
| Both Draft; departure winner enters gateway first | One successful departure, one Conflict, two lease attempts; both callers pending until release, then Accepted; one physical send | `a_lost_departure_does_not_send_again` (WinnerFirst) |
| Both Draft; departure loser enters gateway first | Same evidence; winner reloads the committed receipt under its lease | `a_lost_departure_does_not_send_again` (LoserFirst) |
| Both Draft; either first sender holds the lease, competing wait returns Unavailable | Waiter returns durable Unknown(NotAdmitted); physical sender stays pending; after release Accepted is durable and the waiter can read it | `a_failed_lease_wait_returns_the_durable_departure` |
| Second initial read occurs after departure and gateway entry; receipt is absent | Second caller returns Unknown(NotAdmitted) with no lease attempt or send; first returns Accepted after release; later read adopts it | `a_sender_first_reading_departure_can_return_before_the_send` |
| Entered sender times out while the main thread waits for a missing contender; sender panic poisons the release mutex, then the ordering assertion unwinds | Release cleanup recovers that guard and sets the release flag; the original ordering failure is retained without a second panic aborting the binary | `a_poisoned_sender_release_preserves_the_ordering_failure` |

The failed-wait port fixture injects `StoreError::Unavailable` at the existing
store seam. The SQLite adapter's bounded-wait test separately holds a real writer
lease, gives the competing connection a short busy timeout, observes Unavailable,
and verifies read/reacquisition after release. Production lease timing is unchanged.

## Restoration replays the live transition

Restoration does not keep a second list of legal outcomes. `history_reaches`
starts from the held draft. Departed is `PhoneRecord::depart`. Each later
answer stays only when `budget`, `apply_send`, `apply_lookup`, or
`apply_retry` returns that cause and that outcome. The row's outcome, send
count, and departure must equal that replay
(`a_send_count_the_replay_does_not_reach_is_corrupt`). Lookup and
ReturnedToDraft for one `turn_running` share one initiator. A history this
replay does not return is corrupt. The row is not repaired.

The successor chain is one path. `confirm` is the only walk, and `load` and
`list` use it. A node is visiting while the walk is on it and visited when
the walk has finished it. A live successor edge into a visiting node is a
cycle, including two commands that name each other
(`a_successor_cycle_is_corrupt`). A row whose successor is its own id is that
cycle on one row (`a_self_successor_is_corrupt`). The `ReturnedToDraft` event
is the lineage. The live `successor` column is that edge only while the row's
outcome is still the return, and it names the same id
(`a_successor_names_the_draft_the_return_minted`). A direct answer clears the
live column. A successor that is still only held is deleted in that write
(`a_direct_answer_clears_the_turn_running_successor`). A successor that has
already departed stays. Its return event is still the lineage, so `list`
loads both rows
(`a_departed_recovery_successor_keeps_its_lineage`). A recovery hold resolves
to exactly one command whose return event names it, the prompt matches, and
that command's history reaches its current row. The live column, when it is
still set, is that same command. The walk checks this link in place. It does
not follow the return backward as a second successor edge
(`a_recovery_draft_requires_its_turn_running_predecessor`).

A direct answer that replaces a stored answer is replayed from the row before
the suffix it replaces. That suffix is a lookup, or a lower-ranked direct
answer. The replaced events stay
(`a_direct_answer_after_a_lookup_still_loads`,
`a_stronger_direct_answer_replaces_a_weaker_one`).
A lower or equal direct answer does not replace a stored one, and a history
that appends one does not reach.

A stronger answer from the same replay may depend on a lost or unreadable
send, and the answer may be a returned-to-draft. Those steps are one commit.
The fold starts at the committed row before the direct suffix the answer
replaces. Each step is the transition `apply_send` or `apply_lookup` would
persist from the row before it. The send count on the rolled-back snapshot
is not copied. Copying that count, or teaching restoration to accept it,
leaves the snapshot behind the fold. Writing the return as its own step
drops the lost send the fold counts
(`a_replay_writes_the_lost_send_before_the_stronger_answer`,
`a_replay_rebases_send_counts_onto_the_committed_history`,
`a_replay_counts_the_lost_send_before_turn_running`).
A lost send with no later answer that outranks the stored row is not written,
and a history that appends one does not reach.

The committed history owns the send count when an answer replaces a competing
row. A snapshot from a rolled-back lease is not a counter authority: another
explicit retry may have reset the budget since that snapshot. Replacements ask
the domain to reconstruct the answer from the committed state before the suffix
it replaces. Lookups use the committed state without adding a send. Direct
answers apply the same send transition as restoration. This also applies to a
replayed `turn_running` answer before either minting or saving its refusal.
Taking the maximum of the stale and committed counts is rejected: it can return
success for a row whose history does not reach its count. `rebase_answer` owns
this reconstruction for the application replacement paths.

| Committed row when replay starts | Rolled-back lease | Durable result | Send count the fold reaches |
| --- | --- | --- | --- |
| Direct accepted, one send | Lost send, then done | The lost send, then done. The accepted event stays | 2 |
| Direct accepted after its own lost send | Lost send, then done | Another lost send, applied to the row before that accepted answer, then done | 3 |
| Direct unresolved, one send | Lost send, then direct `turn_running` | The lost send, then the return and its draft | 2 |
| Departed, not admitted | `turn_running`, and the draft id cannot be minted | The failed row. No draft. The caller hears `Id` | 1 |
| Departed, not admitted, after a lost send | `ForegroundSend` `turn_running`, the lease commit is uncertain, and the replay cannot mint | The failed row. The cause is `ForegroundSend`. No draft. The caller hears `Id` | 2 |
| Direct unresolved, one send | Lost send, then `ForegroundSend` `turn_running`, the lease commit is uncertain, a weaker direct answer lands first, and the replay cannot mint | The lost send, then the failed row. The cause is `ForegroundSend`. No draft. The caller hears `Id` | 2 |
| Not admitted | Explicit retry sends `turn_running`, the lease commit is uncertain, and the replay cannot mint | The failed row. The cause is `ExplicitRetry`. No draft. The caller hears `Id` | 1 |
| Lookup failed, a lookup answer, or a weaker direct answer | An explicit retry's lookup, reset, and direct answer roll back; the competing row lands before replay | Replay the not-found lookup and the reset through `apply_retry`, then the winning answer in one commit. The competing events stay. A stronger stored direct answer stays instead | 1 after the reset, plus any lost sends in that retry |
| Lookup failed after a competing recovery write or retry reset | Recovery observes `turn_running` and loses its compare-and-swap, with or without a minted draft | Reload and apply the existing precedence. Save the refusal when it wins; preserve a stronger stored answer and any existing successor. Failed mint returns `Id` after the save | The reloaded committed count, including a concurrent retry's reset; recovery does not send |
| A competing retry resets the budget and stores a weaker direct answer | A direct done or `turn_running` answer at the old count rolls back, then replays | Reconstruct the direct answer from the committed history's reset. A minted draft and a failed mint use that same reconstruction | One send after the committed reset |
| Departed, not admitted | `turn_running`, the draft id cannot be minted, and the open lease commit is `Unavailable` | The departed row stays. The caller hears `Unsaved`. A later send asks the gateway | 0 |

An intent row that is gone while its events remain is corrupt. The id is not
held again. Listing and recovery report that corruption and do not return the
other rows (`an_orphaned_audit_history_is_corrupt`).

| Function | When the outbox asks | What it persists |
| --- | --- | --- |
| `PhoneRecord::depart` | A draft is sent. This commit finishes before the lease | Unknown, not admitted, cause Departed. The person |
| `budget` | Foreground is entered and the send count is already at the budget | Not admitted stays not admitted, cause Bound. Nothing is sent. Lookup failed writes nothing |
| `apply_send` | Foreground calls `conversation.send`. The outcome is not admitted or lookup failed, and the budget remains. The cause argument is SendAnswered, ForegroundSend, or ExplicitRetry | The send count increases by one. A terminal answer keeps that cause and is direct. Lost persists SendLost and keeps the outcome, then a lookup that may resend. Broken persists Unreadable and keeps the outcome, then a lookup that may not resend. `turn_running` returns the failed row. The outbox writes ReturnedToDraft, then Held on the new draft, with the caller's initiator on both |
| `apply_lookup`, resend false | Reconcile, recovery, and the lookup after Unreadable | A changed outcome persists Lookup: a submit receipt, a refusal, a lost or broken lookup, and not-found that moves lookup failed to not admitted. Not found, lost, or broken on attempted persists nothing: attempted stays attempted and is not retryable. `turn_running` persists Lookup, then ReturnedToDraft, then Held on the new draft. One initiator is used for all three |
| `apply_lookup`, resend true | The lookup after SendLost in that same foreground call | Not found under the budget persists Lookup only when the outcome changes, then the next send is ForegroundSend. Not found at the budget persists Bound when the row is already not admitted. A not-found lookup that moves lookup failed to not admitted persists Lookup, including at the budget. `turn_running` persists Lookup, then ReturnedToDraft, with that same initiator. Any other changed outcome persists Lookup |
| `apply_retry` | Explicit retry after a lookup found nothing. Only not admitted or lookup failed | Lookup of not admitted, then ExplicitRetry that sets the send count to zero. The send after that reset is ExplicitRetry |

Departed, SendAnswered, SendLost, Unreadable, ForegroundSend, ExplicitRetry,
and Bound are the person. Lookup and ReturnedToDraft are the person or
recovery. Held is the person, including the draft a direct `turn_running`
mints. Recovery holds only the successor draft recovery minted after a
lookup `turn_running`.

A recovery hold is accepted only when exactly one predecessor names this
draft as its successor, that predecessor's own history reaches it, the
prompt matches, and the predecessor is a `turn_running` return
(`ReturnedToDraft`). A count of rows is not that check
(`a_recovery_draft_requires_its_turn_running_predecessor`).

An audit column is the same tagged union as `phone_intent`. `failed` carries
a failure and a null unknown. `unknown` carries an unknown and a null
failure. Every other outcome has both null. A contradiction is corrupt at
decode. The extra column is not dropped
(`an_event_column_contradiction_is_corrupt`).

Attempted followed by Lookup of not admitted, or of lookup failed, does not
reach (`every_transition_and_precedence_pair`). `after_lookup` keeps
attempted for not found, lost, and broken, so those events are not a replay.

A submit receipt is stage `accepted`, `attempted`, or `settled` with
`dispatched`, `no_effect`, `refused_after_save`, or `unavailable_after_save`.
Stage `ready`, and a settled receipt that omits its effect or carries
`withdrawn`, `cancelled`, or `already_final`, are not outcomes. `no_effect`
is a submit effect and is failed, as row L4. The domain refuses the others.
The row stays. They are never stored as attempted. The wire decoder asks
that same rule (`a_receipt_result_keeps_found_stage_and_effect_together`).
The phone gateway closes the session
(`an_impossible_receipt_closes_the_session`). A custom gateway cannot store
them as attempted.

## Precedence

One order. A direct `conversation.send` answer beats any lookup result,
including a lookup of done and a lookup refusal. A forbidden or failed
lookup never replaces accepted, running, or done.

| Incoming | Stored | Result |
| --- | --- | --- |
| Direct answer | Any lookup result, or any unknown | The direct answer is saved |
| Direct answer | A direct answer of equal or higher outcome | The stored answer stays. Done stays above running, accepted, and failed |
| Lookup, including done and forbidden | A stored direct answer | The stored direct answer stays. Lookup done does not replace direct accepted or a direct refusal |
| Lookup receipt | Unknown, or a lower lookup outcome. No direct answer was stored | The lookup is saved. Rows L3 and R2 are this case |
| Lookup of done | Lookup of accepted, running, failed, or unknown | Done |
| Lookup refusal, including forbidden | Accepted, running, or done, from either source | The stored row stays |
| Lookup of `submission_unresolved` | Unknown, or attempted | Unknown, unresolved. Same as a direct answer. Not failed |
| Lookup refusal | Unknown, or attempted | Failed, with that code |
| Save loses, and the reload returns a row | — | That row, unless the incoming direct answer beats it |
| Save loses, and the reload fails | — | Nothing is written. `Unsaved`. Maybe sent. Never not sent |

Outcome order inside one source, highest first: done, running, accepted,
failed, unresolved, attempted, lookup failed, not admitted, draft.

The owner of this order is the domain. The table-driven test
`every_transition_and_precedence_pair` checks every transition above and
every cell of this table.

## Scenario index

The rows below name the regression for each ordering. They do not add a
second rule.

| Row | Ordering | Result | Test |
| --- | --- | --- | --- |
| H1 | Text saved, no send | Draft. Nothing is asked of the gateway | `a_saved_draft_is_not_a_submission` |
| H2 | Process drops after the save, then the store reopens | The same draft, still not a submission | `a_draft_survives_reopen_without_a_send` |
| H3 | Same command id, different text or images | The original bytes stay. Nothing is sent | `a_changed_body_under_the_same_command_is_refused` |
| H4 | Same bytes saved again | The original draft, no second row | `saving_the_same_bytes_again_keeps_one_draft` |
| H5 | Two holders save the same bytes at once | One draft. Both calls return it | `a_concurrent_hold_of_the_same_bytes_keeps_one_draft` |
| S1 | Send starts | Departed is durable before `conversation.send` is asked | `departure_is_durable_before_the_gateway_is_asked` |
| S2 | That save's commit is uncertain | `conversation.send` is not asked | `an_uncertain_departure_does_not_send` |
| S15 | The lease commit is uncertain after SendLost, a lookup that changes the outcome, and ForegroundSend | Those events are written again, in order. The row loads | `an_uncertain_lease_rewrites_every_transition` |
| S12 | Two callers load the same Draft before departure | The waiter blocks on the lease and reloads. It sends the same id only when the stored row is still unknown | `a_lost_departure_does_not_send_again` |
| S19 | The departure commit wins, then the other caller holds the lease and stores a direct answer | The winner reloads inside the lease and does not send | `the_departure_winner_reloads_before_it_sends` |
| S3 | Send returns queued, injected, or settled | Accepted, running, or done | `a_queued_send_is_accepted`, `an_injected_send_is_running`, `a_settled_send_is_done` |
| S4 | Send returns a conversation refusal other than `submission_unresolved` | Failed. The same id is not sent again | `a_refusal_is_failed_and_not_sent_again` |
| S5 | Send returns `turn_running` | Failed. The text is a new draft under new ids | `turn_running_returns_the_text_to_a_new_draft` |
| S6 | Send returns `forbidden` or `drive_policy_unenforceable` | Failed. The same id is not sent again | `a_lowered_grant_stays_failed`, `an_unenforceable_drive_policy_stays_failed` |
| S7 | Send returns `conversation_deleted` | Failed. The text stays readable | `a_deleted_conversation_keeps_the_typed_text` |
| S8 | Send returns `submission_unresolved` | Unknown, unresolved. The same id is not sent again | `submission_unresolved_is_not_sent_again` |
| S9 | Send answer is not a receipt or a known refusal | The receipt is read. No further send in that call | `an_unreadable_send_answer_is_not_sent_again` |
| S10 | Saving the answer fails | Maybe sent. The caller does not hear not sent | `a_failed_save_after_a_send_keeps_possible_delivery` |
| S11 | A direct answer arrives against a stored lookup, including forbidden and done | The direct answer is saved | `a_direct_answer_beats_a_lookup` |
| S16 | An uncertain answer commit rolls back, and a lookup of done is saved before the direct answer is written again | The direct answer loads. The lookup event stays | `a_direct_answer_after_a_lookup_still_loads` |
| S20 | That rollback lets another caller store a lower direct answer before the stronger one is written again | The stronger answer loads. The lower direct event stays | `a_stronger_direct_answer_replaces_a_weaker_one` |
| S21 | That stronger answer was a second send after SendLost | SendLost is written in the same commit, then the stronger answer. The row loads at that send count | `a_replay_writes_the_lost_send_before_the_stronger_answer` |
| S22 | The stored direct answer is also a second send, and the rolled-back answer is a two-send done | The replayed lost send is applied to the row before the stored answer. The row loads at 3 | `a_replay_rebases_send_counts_onto_the_committed_history` |
| S23 | The rolled-back lease is SendLost, then direct `turn_running`, and a lower direct answer is stored first | SendLost is written in that commit, then the return. The row loads at 2 | `a_replay_counts_the_lost_send_before_turn_running` |
| S24 | `turn_running` is observed and the draft id cannot be minted | The failed row is committed. No draft. The same id is not sent again | `a_turn_running_refusal_is_saved_when_the_draft_id_fails`, `a_lookup_turn_running_is_saved_when_the_draft_id_fails` |
| S25 | That observation is a `ForegroundSend` or `ExplicitRetry` whose lease commit is uncertain, and the replay cannot mint | The failed row keeps that cause. No draft | `a_replayed_foreground_send_keeps_its_cause_when_the_draft_id_fails`, `a_replayed_explicit_retry_keeps_its_cause_when_the_draft_id_fails` |
| S26 | S25, and a weaker direct answer is stored before the replay | The lost send is written, then the failed row. The cause stays `ForegroundSend` | `a_replayed_turn_running_keeps_its_cause_when_a_weaker_answer_landed_first` |
| S27 | The open lease commit returns `Unavailable` after `turn_running` and a failed mint | The refusal is not durable. The caller hears `Unsaved` and a later send asks the gateway | `a_failed_lease_commit_after_turn_running_is_unsaved` |
| S28 | Recovery observes `turn_running` and the draft id cannot be minted | The failed row is committed. The caller hears `Id`. No lease was open | `a_recovery_turn_running_is_saved_when_the_draft_id_fails` |
| S29 | An explicit retry rolls back and a competing lookup or direct answer lands before replay | The lookup and reset precede the winning direct answer in one commit; its history reloads, with the reset count | `a_replayed_explicit_retry_keeps_its_prefix_after_a_competing_answer` |
| S30 | Recovery observes `turn_running`, minting fails, and a competing write wins the first save | Reload and apply precedence, then return `Id`. A weaker lookup cannot erase the refusal; stronger evidence and existing successors stay | `a_recovery_mint_failure_reloads_after_a_competing_lookup` |
| S31 | Recovery reads a spent send count, then another caller retries, resets that count and commits lookup failed before recovery saves `turn_running` | Both a failed mint and a successful mint preserve the reset committed count. The refusal and any new draft reload | `a_recovery_refusal_uses_the_committed_count_after_a_retry_reset`, `a_recovery_draft_uses_the_committed_count_after_a_retry_reset` |
| S32 | A sender reloads count two and receives a direct answer at count three; its lease rolls back, then a retry resets the count and stores a weaker direct answer at one before replay | The winning receipt or refusal reloads at the count reconstructed from the committed reset, including successful and failed minting | `a_direct_replay_rebases_after_a_concurrent_retry_reset`, `a_returned_replay_rebases_after_a_concurrent_retry_reset`, `a_refusal_replay_rebases_after_a_concurrent_retry_reset` |
| S17 | That lookup is `turn_running` and has already minted a draft | The direct answer is saved, the successor is cleared, and the still-held draft is removed | `a_direct_answer_clears_the_turn_running_successor` |
| S14 | A lookup arrives after a direct answer was stored | The direct answer stays. This is the order the lease produces | `a_lookup_does_not_replace_a_direct_answer` |
| L1 | Send answer lost, lookup finds `accepted` | Accepted. No further send | `a_lost_answer_with_an_accepted_receipt_is_not_sent_again` |
| L2 | Send answer lost, lookup finds `attempted` | Unknown, attempted. No further send | `an_attempted_receipt_is_unknown_and_not_sent_again` |
| L3 | Send answer lost, lookup finds settled `dispatched` | Done | `a_dispatched_receipt_is_done` |
| L4 | Lookup finds settled `no_effect`, `refused_after_save`, or `unavailable_after_save` | Failed with `revoked`, `revoked_after_save`, or `unavailable_after_save` | `a_settled_refusal_is_failed` |
| L5 | Send answer lost, lookup finds nothing, foreground budget remains | The same bytes are sent again. The resend is recorded with its answer | `a_lost_answer_retransmits_the_same_bytes_until_the_bound` |
| L6 | That budget is spent | Unknown, not admitted, cause Bound. No further send in that call | `foreground_send_stops_at_the_bound` |
| L7 | Send answer lost and the lookup itself fails | Unknown, lookup failed | `a_lookup_that_cannot_be_asked_stays_unknown` |
| L10 | That lookup failed row is explicitly retried and the lookup finds nothing | Not admitted, then the same bytes. The row reloads | `a_lookup_failure_is_retried_as_an_explicit_send` |
| L11 | A later foreground call loses a send of lookup failed, then finds nothing | Lookup of not admitted, then ForegroundSend | `every_transition_and_precedence_pair` |
| L9 | A waiter reaches lookup failed after that budget is already spent | Stays lookup failed. No send and no Bound event. The row still loads | `a_spent_budget_keeps_lookup_failed` |
| L8 | Lookup returns `submission_conflict` | Failed. `found` is not a receipt | `a_conflicting_lookup_is_failed` |
| R1 | Restart after departure, lookup finds nothing | Unknown. No send | `recovery_does_not_send_when_the_receipt_is_absent` |
| R7 | Restart after lookup failed, lookup finds nothing | Not admitted. No send | `a_recovery_of_lookup_failed_that_finds_nothing_is_not_admitted` |
| R2 | Restart after departure, lookup finds settled `dispatched` | Done. No send | `recovery_adopts_a_dispatched_receipt_without_sending` |
| R6 | Restart after attempted, lookup finds settled `dispatched` | Done. No send | `recovery_adopts_a_settled_receipt_after_attempted` |
| R3 | Explicit retry after a lookup that found nothing | Same bytes, under the lease | `an_explicit_retry_looks_up_before_it_sends` |
| R5 | Two retries share the file | One send. The waiter reloads | `a_concurrent_retry_sends_the_departure_once` |
| R4 | Explicit retry of unresolved, attempted, failed, or done | No send | `an_explicit_retry_does_not_send_an_uncertain_or_finished_command` |
| C1 | Stored canonical bytes do not decode | Corrupt. The row is not rewritten | `corrupt_canonical_bytes_are_left_in_place` |
| C3 | Events do not reach the row, or a send cause is marked recovery | Corrupt. The row is not rewritten | `a_done_row_whose_history_stops_at_departure_is_corrupt` |
| C4 | A lookup event claims running | Corrupt. The row is not rewritten | `a_lookup_cannot_restore_running` |
| C5 | Attempted is followed by not admitted or lookup failed | Corrupt. The attempt is not retried | `every_transition_and_precedence_pair`, `an_attempted_receipt_is_not_restored_as_not_admitted` |
| C6 | A recovery hold whose return event is missing, not unique, a different prompt, whose history does not reach, or not the recovery caller | Corrupt. The draft is not sent | `a_recovery_draft_requires_its_turn_running_predecessor` |
| C7 | An event's failure or unknown column contradicts its outcome | Corrupt. The column is not dropped | `an_event_column_contradiction_is_corrupt` |
| C8 | The stored send count disagrees with the replay | Corrupt. The count is not rewritten | `a_send_count_the_replay_does_not_reach_is_corrupt` |
| C9 | The successor column names a different command than the return event | Corrupt. The column is left | `a_successor_names_the_draft_the_return_minted` |
| C10 | The intent row is gone and its events remain | Corrupt. The id is not held again. List and recovery do not return the other rows | `an_orphaned_audit_history_is_corrupt` |
| C11 | Successor edges cycle, including two commands that name each other | Corrupt. The columns are left | `a_successor_cycle_is_corrupt` |
| S18 | A direct answer replaces a lookup after the recovery draft has departed | The draft stays. The return event is its lineage. Both rows load | `a_departed_recovery_successor_keeps_its_lineage` |
| C2 | Each transition | Command, outcome, cause, and initiator | `a_transition_keeps_its_cause_and_initiator` |

`FOREGROUND_SENDS` in `crates/nessa-client-core/src/outbox/domain/decide.rs`
is the foreground budget, the first send included. The limits catalogue names
that constant and does not copy the count.

## What this ordering does with the contract

The five rendered outcomes are the contract's. A few facts around them are
fixed here so a review can see them without editing the contract:

- Receipts and Stop from #268 are on main through pull request 537. The
  contract's note that `codex/268-command-receipts` and
  `codex/268-semantic-consumer` are absent from main describes those branch
  tips, which are behind main. This work does not merge them.
- `forbidden` and `unauthorized` are socket codes. They stay on `failed`.
  They are not a sixth outcome.
- `turn_running` is `failed`, and the typed text is copied onto a new draft
  under new ids. The original id is not sent again.
- A typed lookup refusal, including a deleted conversation, is `failed`.
  `unknown` is maybe sent: a lost send, a lookup that did not finish, an
  `attempted` stage, an unreadable answer, or `submission_unresolved`
  from either the send or the lookup. It is not a report that nothing was
  sent.
- A settled submit receipt that omits its effect, or that carries
  `withdrawn`, `cancelled`, or `already_final`, is not a submit receipt.
  The domain leaves the row unchanged. Settled `no_effect`,
  `refused_after_save`, and `unavailable_after_save` are failed, as row L4
  says.
- Every other conversation code, including `temporarily_unavailable`, stays
  `failed` and is not retried under the same id.
- `conversation.send` and `conversation.receipt` are product calls on the
  pinned session. They are not reads through `RetainedCache`.

The phone application and the desktop saved-tabs caller are not wired here.
Cedar still admits the session that carries the two calls.
