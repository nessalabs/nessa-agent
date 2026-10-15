# Phone outbox

Issue: [#269](https://github.com/nessalabs/nessa-agent/issues/269).
The send id, the receipt lookup, the five rendered outcomes, and replay are
the [phone send contract](https://github.com/nessalabs/nessa-agent/blob/cursor/owner-phone-authorization-5058/docs/design/phone-send-contract.md)
(pull request 885). This page is the device
ordering around that contract. The contract is not restated here and is not
edited by this work.

The outbox lives in `nessa-client-core`. It saves the typed message and its
command id before the first `conversation.send`. A draft has not been
submitted. After a send may have left the device, a restart or a later call
reads `conversation.receipt` and does not send. Only the call that is still
in the foreground, and an explicit retry after that lookup found nothing,
may send again, and only up to `FOREGROUND_SENDS`.

Cedar admits the session. This outbox does not decide who may send.

| Row | Ordering | Result | Test |
| --- | --- | --- | --- |
| H1 | Text saved, no send | Draft. Nothing is asked of the gateway | `a_saved_draft_is_not_a_submission` |
| H2 | Process drops after the save, then the store reopens | The same draft, still not a submission | `a_draft_survives_reopen_without_a_send` |
| H3 | Same command id, different text or images | The original bytes stay. Nothing is sent | `a_changed_body_under_the_same_command_is_refused` |
| H4 | Same bytes saved again | The original draft, no second row | `saving_the_same_bytes_again_keeps_one_draft` |
| H5 | Two holders save the same bytes at once | One draft. Both calls return it | `a_concurrent_hold_of_the_same_bytes_keeps_one_draft` |
| S1 | Send starts | Departed is durable before `conversation.send` is asked | `departure_is_durable_before_the_gateway_is_asked` |
| S2 | That save's commit is uncertain | `conversation.send` is not asked | `an_uncertain_departure_does_not_send` |
| S12 | Two sends depart the same draft | One `conversation.send`. The loser reads the receipt and does not send | `a_lost_departure_does_not_send_again` |
| S3 | Send returns queued, injected, or settled | Accepted, running, or done. The text stays | `a_queued_send_is_accepted`, `an_injected_send_is_running`, `a_settled_send_is_done` |
| S4 | Send returns a conversation refusal other than `submission_unresolved` | Failed, with that code. The same id is not sent again | `a_refusal_is_failed_and_not_sent_again` |
| S5 | Send returns `turn_running` | Failed with that code. The typed text is a new draft under new ids. The original id is not sent again | `turn_running_returns_the_text_to_a_new_draft` |
| S6 | Send returns `forbidden` or `drive_policy_unenforceable` | Failed with that code, text still readable, same id not sent again | `a_lowered_grant_stays_failed`, `an_unenforceable_drive_policy_stays_failed` |
| S7 | Send returns `conversation_deleted` | Failed with that code. The typed text remains readable | `a_deleted_conversation_keeps_the_typed_text` |
| S8 | Send returns `submission_unresolved` | Unknown, cause unresolved. The same id is not sent again | `submission_unresolved_is_not_sent_again` |
| S9 | Send answer is not a receipt or a known refusal | The receipt is read. The same bytes are not sent again in that call | `an_unreadable_send_answer_is_not_sent_again` |
| S10 | The send is answered and saving that answer fails | The error says the message may have been delivered. The saved row stays unknown and does not claim submission. A later send reads the receipt | `a_failed_save_after_a_send_keeps_possible_delivery` |
| S11 | Another connection commits an outcome while `conversation.send` is in flight | This call returns that outcome when it is at least as strong as this receipt. The command id stays | `a_reconciled_row_is_kept_when_the_send_save_loses` |
| S13 | That committed outcome is a weaker unknown, and this send returned a receipt | The receipt is saved. The command is not left retryable | `a_queued_receipt_replaces_a_weaker_lookup_failure` |
| L1 | Send answer lost, lookup finds `accepted` | Accepted. No further send | `a_lost_answer_with_an_accepted_receipt_is_not_sent_again` |
| L2 | Send answer lost, lookup finds `attempted` | Unknown, cause attempted. No further send | `an_attempted_receipt_is_unknown_and_not_sent_again` |
| L3 | Send answer lost, lookup finds settled `dispatched` | Done | `a_dispatched_receipt_is_done` |
| L4 | Lookup finds settled `no_effect`, `refused_after_save`, or `unavailable_after_save` | Failed with `revoked`, `revoked_after_save`, or `unavailable_after_save` | `a_settled_refusal_is_failed` |
| L5 | Send answer lost, lookup finds nothing, foreground budget remains | The same canonical bytes are sent again. The resend is recorded with its answer | `a_lost_answer_retransmits_the_same_bytes_until_the_bound` |
| L6 | That budget is spent | Unknown, cause not admitted. No further send in that call | `foreground_send_stops_at_the_bound` |
| L7 | Send answer lost and the lookup itself fails | Unknown, cause lookup failed. No further send | `a_lookup_that_cannot_be_asked_stays_unknown` |
| L8 | Lookup returns `submission_conflict` | Failed with that code. `found` is not treated as a receipt | `a_conflicting_lookup_is_failed` |
| R1 | Restart after departure, lookup finds nothing | Unknown. `conversation.send` is not asked | `recovery_does_not_send_when_the_receipt_is_absent` |
| R2 | Restart after departure, lookup finds settled `dispatched` | Done. `conversation.send` is not asked | `recovery_adopts_a_dispatched_receipt_without_sending` |
| R6 | Restart after an attempted receipt, lookup finds settled `dispatched` | Done. `conversation.send` is not asked | `recovery_adopts_a_settled_receipt_after_attempted` |
| R3 | Explicit retry after a lookup that found nothing | The lookup is recorded as unknown, not admitted, then one new foreground budget of the same bytes. The writer lease is held across that send | `an_explicit_retry_looks_up_before_it_sends` |
| R5 | Two retries of one departed command share the file | One send. The waiter reads the committed outcome | `a_concurrent_retry_sends_the_departure_once` |
| R4 | Explicit retry of unresolved, attempted, failed, or done | No send | `an_explicit_retry_does_not_send_an_uncertain_or_finished_command` |
| C1 | Stored canonical bytes do not decode | Corrupt. The row is not rewritten | `corrupt_canonical_bytes_are_left_in_place` |
| C3 | A done row whose events stop at departure | Corrupt. The row is not rewritten | `a_done_row_whose_history_stops_at_departure_is_corrupt` |
| C2 | Each transition | The event records the command, the outcome, the cause, and the initiator. `hold`, `send`, and `retry` record the person. `recover` records recovery | `a_transition_keeps_its_cause_and_initiator`, `recovery_adopts_a_dispatched_receipt_without_sending` |

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
  `unknown` is a lost send whose lookup found nothing, a lookup that did not
  finish, an `attempted` stage, an unreadable answer, or
  `submission_unresolved`. A settled submit receipt that omits its outcome,
  or that carries `withdrawn`, `cancelled`, or `already_final`, is a protocol
  failure. Settled `no_effect`, `refused_after_save`, and
  `unavailable_after_save` are failed, as row L4 says.
- Every other conversation code, including `temporarily_unavailable`, stays
  `failed` and is not retried under the same id.
- `conversation.send` and `conversation.receipt` are product calls on the
  pinned session. They are not reads through `RetainedCache`.

The phone application and the desktop saved-tabs caller are not wired here.
Cedar still admits the session that carries the two calls.
