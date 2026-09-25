# 182. Archiving and deleting conversations

## Purpose

Let a person put a conversation out of sight, or get rid of it for good, from
the panel's Messages list — and settle what "for good" erases, what it keeps,
and what Nessa cannot reach.

- **Date:** 2026-09-24
- **Status:** proposed — implemented on the branch that adds the Messages list
  ([#181](https://github.com/nessalabs/nessa-agent/issues/181)), pending review.
- **Issue:** [#182](https://github.com/nessalabs/nessa-agent/issues/182)

## Two different requests

**Archive** is a filing decision, and only for a conversation the gateway has
a summary for — one somebody said something in, whose summary was written:
those are the conversations the list shows, so archiving one without a summary
(nothing was said in it, or its summary was never written) changes nothing
(`applied: false`). A conversation
whose summary cannot be read had something said in it, so it is listed — bare,
and in the default list, since whether it was archived cannot be read; showing
it where people look first never loses it from view. The conversation
is untouched: its history, its uploads and its agent stay exactly as they
were, and `conversation.list`
simply stops showing it unless archived conversations are asked for. It is
undone by unarchiving, and by talking in it again — a conversation somebody is
writing in is not archived, which is the rule Mail and Messages follow. The flag
lives in the conversation's summary, but unlike the rest of the summary it is a
person's decision rather than a projection, so a failure to record it is a
failure of the command, not a stale list.

**Delete** is permanent. It is a consequential transition under
[audit evidence is part of the behavior](../../../CODING_STANDARDS.md#audit-evidence-is-part-of-the-behavior),
and it is decided here in that light.

## What delete does, in order

1. **Authorize** from the ownership record, as `conversation.close` does.
2. **Fence.** A durable tombstone for the conversation's identity is written
   before anything is stopped or removed. From then on every command naming it
   — including `conversation.create`, which surfaces send before every other
   command and which would otherwise recreate the conversation under the same
   identity — is refused with `conversation_deleted`. The tombstone is a file
   of its own beside the ownership record (`metadata/deleted/<id>.json`), and
   both are kept for good: an identity is minted once and never reused
   ([gate 12](../../../CODING_STANDARDS.md#gates)). The tombstone holds the first
   decision — who deleted it, from which surface, in answer to which request,
   and when — and, once read, the provider session, so a repeated delete
   (another request, even after the history is gone) records the deletion in
   the first request's name rather than contradicting it. The deletion is dated
   by the gateway's clock, and never earlier than the conversation's creation:
   a clock stepped back since then would otherwise propose a tombstone the
   conversation's own record refuses on read. Only the owner is told the
   conversation was deleted; anyone else is told it was not found.
3. **Stop** the live agent exactly as close does. If the stop is not confirmed,
   nothing is erased; the tombstone stands and the delete is finished later.
4. **Ask the agent to delete its own session.** Nessa talks to agents over ACP,
   and each agent keeps its own record of a session. The provider session read
   from our saved history is handed to one authority — a registry of erasers
   keyed by agent, filled in composition: the fixed agents where their
   providers are built, and OpenCode by its current generation, observed afresh
   on each ask as a cold conversation's provider is (not installed or without a
   credential now is an agent not built this run) — which dispatches to that
   agent's handler, kept in the agent's own SDK module. Each launch reads the
   agent's credential and holds its executable in use as an opening does. Each opens a connection
   of its own without resuming the session, and sends `session/delete` only if
   the agent advertises it. The delete is always sent first when it can be,
   and an accepted delete depends on nothing else, since only the agent knows
   whether it still has the session: Claude's list leaves out a session with
   no titled prompt, so a conversation of images alone is never listed. Only
   when the agent refuses, and it advertises `session/list`, is its list for
   the workspace the agent runs in read, to interpret the refusal: a refusal of
   a session the list, read in full, does not name is what the agent says of
   one it no longer has, and is recorded as `not_listed`, so an interrupted
   deletion can finish — Claude's delete of a session already gone is an error
   with no code of its own, and a retry must not depend on reading its
   message. Any other refusal stays a failure, asked again: the list names the
   session, or could not be read in full (refused, past its budget, too large,
   a page without `sessions`, an entry without a string `sessionId`, a
   `nextCursor` that is neither a string nor null or repeats one already followed, too
   many pages), and nothing is claimed about a session it could not speak
   for. What the agent confirmed is recorded as it said it:
   Claude's adapter deletes the session (`deleted`), Codex's archives the
   thread (`archived`), Opencode's acknowledges (`acknowledged`); an agent that
   does not advertise deletion is `not_supported`, and one this build has no
   adapter for is `no_handler`. An agent this build can run but did not start
   this time (its runtime missing, its configuration refused) is not
   `no_handler`: the deletion stays unfinished until the agent is back.
   `not_listed` says only that the agent refused and did not list it, not
   that it erased it. Removing an agent is deleting its module and its one
   registration. A failure leaves the deletion unfinished, to be finished later
   — a handler that panics while asked included: the delete still happened and
   is answered `conversation_erasure_incomplete`, and since nothing this run
   can see will change, it is finished by a repeated delete or the next start,
   not carried on in-process
   — and a refusal the agent keeps giving keeps it unfinished for as long as it
   gives it, with Nessa's own history and summary kept and no deletion record
   written
   (see [What Nessa cannot erase](#what-nessa-cannot-erase)).
5. **Record** who deleted what: the conversation, organization and owner; the
   state before and after; the cause; the initiating principal and surface; the
   request that asked for it; when it was requested and observed; and the
   provider session identity read from the saved history, which is the only
   link from this conversation to audit records keyed by the provider's session
   and is about to be erased — and what the agent did with it
   (`providerErasure`). If the record cannot be written, necessary
   cleanup still happens but history is not erased, and the command answers
   `audit_unavailable`.
6. **Erase** the saved history (under a session lease the delete holds, so no
   second writer can appear; the lease is waited for briefly, since a stopped
   agent's last handles let go of it just after the stop), the conversation's
   uploads (released with their own per-hold evidence, even when the deletion
   record failed), and its summary. The journal's empty `.lock` file is kept:
   unlinking it while held would let a second opener lock a new file beside it.
   The history and summary are erased only once the agent's answer is settled
   and recorded: Nessa's record of what was said never goes before the
   agent's. Uploads go regardless, since no agent is left to read them.
   Whatever cannot be removed now is reported as
   `conversation_erasure_incomplete`, or `audit_unavailable` when it is the
   deletion record (the table below says which): either way the conversation
   *is* deleted, and the erasure is tried again later (see "Finishing what
   was left" for what needs the operator). The request that decided the
   deletion is answered `applied: true`, as is a repeat of that request from
   the same surface; any other request is answered `applied: false`.

The agent is asked only while this deletion holds the saved history's lease,
so it is never told to delete a session something else is still writing. At
most two agents are asked at once, and a slot is taken only when an agent will
be asked. A delete that finds both taken, its conversation's agent not yet
confirmed stopped within `stopMs` (an agent's own stop is bounded by its
shutdown budgets, which may be longer), or its history still held by another
writer, is left unfinished and finished by the running gateway itself — as soon
as a slot frees, or after a few spaced tries for the stop or the history — so a delete
somebody asked for does not wait for the next start. That finishing never
queues: it passes over a deletion whose attempt is already in progress, since
that attempt is carrying it, so a person's delete is never queued ahead of by
a background try — though a try already running can make it wait one attempt
(row 28 of the table below). The newest reason a deletion waits for is the one it waits on. Deletes have a pool of their own on the socket,
so a burst of them never takes the place of a permission answer, a cancel or a
close. Stopping the gateway — and only that — ends a delete that is stopping
the conversation, waiting for its history or asking its agent; once the
tombstone is written every such ending is answered
`conversation_erasure_incomplete`, and the deletion is finished after the next
start. No agent is launched once the gateway has begun to stop. An exchange
abandoned that way still stops the agent's process and releases what it
launched with: the gateway waits for that before it exits, bounded by each
agent's stop budget (a grace period and four kill timeouts).

**How long a delete takes.** One attempt is bounded by what the budget table
(`protocol/defaults/agent-startup-budgets.json`) states: stopping the
conversation, waiting for its history, then one agent exchange — launch,
delete, listing after a refusal, and a teardown of a grace period and four kill timeouts
(`stopMs + historyLeaseMs + launchMs + 2 × startupMs + shutdownGraceMs +
4 × killTimeoutMs`; the SDK states the exchange's part once, as
`AcpConfig::session_deletion_limit`). A delete that finds another attempt in progress
waits for it and answers from what it recorded, rather than making a second.
The client waits out that bound, from the same table; normally the agent is
warm and a delete answers in seconds, and the panel has taken the row out of
the list already.

**Finishing what was left.** The tombstone records how far a deletion got —
history read, agent asked, erased — so an unfinished one carries on from there
rather than starting again. A repeated delete finishes it, and so does the
gateway itself: after it starts, it finishes every unfinished deletion in the
background (a few bounded attempts, never blocking startup), logging with its
typed cause whatever is still left for the next start — every start, for as
long as the cause lasts: an agent that keeps refusing is asked again each
time, and nothing is erased meanwhile. One that finds both
slots taken, its agent still stopping, or its history held is handed to the same in-process finishing as
a delete somebody asked for, rather than spending those attempts. The deletion record is
always written in the name of the request that decided it.

A journal too damaged to read the provider session from keeps the delete
incomplete. The journal is `s-<encoded-id>.jsonl` — the conversation's
identity as lowercase, unpadded base32hex of its bytes (`SessionPaths` in
`nessa-sdk`'s `session_storage/paths.rs`) — and the gateway logs its full path
when it cannot read it. The operator's remedy is to move that journal out of
the sessions directory, leaving its `.lock` in place: the next delete or
gateway start records the deletion with no provider session and
`providerErasure: "session_unknown"` — never `no_provider_session`, which
would claim the history named none — and finishes. The moved-aside file is
then the only link to the agent's own transcript.

**A conversation record that cannot be read.** Such a record is skipped
wherever records are read: by every list, and by the startup finish, which
counts it in its summary since it may be a deletion that cannot even be seen.
That covers a damaged record, a damaged or contradicting tombstone, a record
whose file is not private to its owner (mode `0600`), and a record written
before records named their agent, which is refused rather than read in an old
shape. Whose it is cannot be read either, so while one is there no list is
`complete` — for every caller on the gateway, not only its owner, and until it
is repaired.

A tombstone whose record is gone — moved aside, say, as below — is counted
apart. It is a deleted conversation, which no list shows anyway, so it leaves
every list as complete as it was; but its deletion can be neither read nor
finished, so the startup finish counts it, and the log names its file. The operator's remedy: run
`node scripts/retrofit-conversation-agents.mjs` once, with the gateway stopped,
for records from before agents were named; for a damaged record, repair it or
move it out of the conversations directory (a deleted one's tombstone then
stands alone, and is counted as above). A damaged tombstone is repaired,
or moved aside together with its record — never alone, which would bring the
deleted conversation back; what that deletion had not erased yet is then the
operator's to remove. The gateway's log names each such file.

### The lifecycle, as a table

This table is the specification of a deletion on the gateway; the prose above
says why; it is kept as [gate 15](../../../CODING_STANDARDS.md#gates) asks, with
the row-to-test mapping after it.

**States.** A conversation's tombstone records how far its deletion got:

| State | Tombstone |
|---|---|
| `live` | none: not deleted |
| `fenced` | written; history `Unread`; no provider erasure |
| `read·none` | history `Absent`; erasure `no_provider_session` (settled in the same step) |
| `read·unknown` | history `Unknown` (a lease with no journal); erasure `session_unknown` (settled in the same step) |
| `read·session` | history `Recorded(session)`; no provider erasure yet |
| `settled` | `Recorded(session)` and an erasure: `deleted`, `archived`, `acknowledged`, `not_listed`, `not_supported` or `no_handler` |
| `erased` | `erased: true`: finished |

Beside the tombstone, the running gateway keeps a retry table for deletions it
carries on itself: none; `ForSlot(g)`, waiting for an agent slot; or
`ForRelease(g, n)`, waiting for a stop or a lease to be let go, after `n` timed
tries. `g` is the generation of the reason: a try's result is recorded only if
no newer reason was recorded meanwhile. A deletion is *held elsewhere* while
another attempt, or a delete reading its tombstone to answer, holds its lock.

**Events and what they do.** "Unfinished" is the answer
`conversation_erasure_incomplete`, except where the row says
`audit_unavailable`; either way the conversation is already deleted. Those two
are the only answers that promise anything on failure: every other error from
a delete says only that it is not known whether it fenced the conversation. "Left" is
a deletion no in-process rule carries on: a repeated delete or the next start
finishes it.

| # | In state | Event | Answer on the wire | Tombstone after | Erased / kept | Carried on |
|---|---|---|---|---|---|---|
| 1 | `live` | Owner's delete; every step succeeds | `applied: true` | `fenced` → read → `settled` → `erased` | agent stopped; deletion record written; uploads, history and summary erased | — |
| 2 | any | Delete by anyone but the owner | `conversation_not_found` | unchanged | nothing | — |
| 3 | `fenced`…`erased` | Any other command, a create included | owner: `conversation_deleted`; others: `conversation_not_found` | unchanged | nothing | — |
| 4 | any | Delete after retirement has begun | `temporarily_unavailable`: not known | none written by this delete | nothing | — |
| 5 | `fenced` | History never opened | continues as row 1 | `read·none` | no agent asked | — |
| 6 | `fenced` | History lease exists, journal gone | continues as row 1 | `read·unknown` | no agent asked | — |
| 7 | `fenced` | History unreadable, or names another session | unfinished | `fenced` | uploads let go; history, summary kept; no deletion record | left |
| 8 | `fenced` | Agent's stop not confirmed within `stopMs` | unfinished | `fenced` | nothing: not even uploads | `ForRelease` |
| 8b | `fenced` | The agent's stop fails outright: its close fails, or its launch's cleanup cannot be confirmed | unfinished | `fenced` | nothing: not even uploads | left |
| 8c | `fenced` | Its opening failed and may still hold what it launched (whatever cleanup it carries) | unfinished | `fenced` | nothing: not even uploads | left |
| 9a | `fenced` or `read·session` | History lease still held after `historyLeaseMs` | unfinished | unchanged | uploads let go; history, summary kept; no deletion record; agent not asked | `ForRelease` |
| 9b | `read·none`, `read·unknown` or `settled` | History lease still held after `historyLeaseMs` | unfinished | unchanged | deletion record written; uploads let go; summary erased; history kept | `ForRelease` |
| 9c | any after `fenced` | The history's lease cannot be opened at all (storage failing, not held elsewhere) | unfinished | unchanged | as row 9a or 9b, by state | left |
| 10 | `ForRelease(g, n<2)` | Its own timer fires; still held | — | unchanged | nothing | `ForRelease(g, n+1)`, due only on its own timer |
| 11 | `ForRelease(g, 2)` | Third timed try still held | — | unchanged | nothing | left (logged) |
| 12 | `read·session` | Both agent slots taken (the gateway's own bound; an eraser's error never counts as this) | unfinished | unchanged | uploads let go; history, summary kept; no deletion record | `ForSlot`, due at once: tried as soon as the worker runs, since a slot may have freed before the wait was recorded |
| 13 | `ForSlot` | A slot frees (an ask ends, however it ends) | — | as the try goes | as the try goes | tried; no try spent; a slot freed during a try is not lost; a try that then finds a release to wait for starts that wait's tries where they stood |
| 14 | waiting | A newer reason is recorded while a try runs | — | — | — | the newer reason stands |
| 15 | waiting | Worker try finds it held elsewhere | — | unchanged | nothing | due again: `ForSlot` after one delay, `ForRelease` on its own timer; no try spent |
| 16 | `read·session` | Agent accepts the delete | continues as row 1 | `settled`: what the binding says (`deleted` / `archived` / `acknowledged`) | as row 1 | — |
| 17 | `read·session` | Agent does not offer deletion | continues as row 1 | `settled` `not_supported` | as row 1 | — |
| 18 | `read·session` | Agent refuses; its list, read in full, does not name the session | continues as row 1 | `settled` `not_listed` | as row 1 | — |
| 19 | `read·session` | Agent refuses otherwise (lists it, list unreadable, no list), or cannot be launched or answer | unfinished | unchanged | uploads let go; history, summary kept; no deletion record | left; asked again by every repeat and start |
| 20 | `read·session` | Agent's handler panics | unfinished | unchanged | as row 19 | left; its slot frees (row 13) |
| 21 | `read·session` | No handler in this build / handler not started this run | continues as row 1 / unfinished | `settled` `no_handler` / unchanged | as row 1 / as row 19 | — / left |
| 22 | `settled` or `read·none`/`read·unknown` | Deletion record refused | `audit_unavailable` | unchanged | uploads let go; history, summary kept | left |
| 23 | `settled` | Uploads' release fails | `audit_unavailable` if only their evidence; otherwise unfinished | unchanged | history, summary erased | left |
| 24 | `settled` | Summary cannot be erased | unfinished | unchanged | the rest erased | left |
| 25 | `settled` | Finished tombstone cannot be written | unfinished | `settled` | everything erased | left |
| 26 | `erased` | Repeat of the deciding request (same principal, surface, `requestId`) / any other delete by the owner | `applied: true` / `applied: false` | unchanged | nothing | — |
| 27 | `fenced`…`settled` | Any repeat delete by the owner | as the attempt goes; `applied` by row 26's rule | carried on from where it stands; the first decision stays | nothing written down is read or asked again (row 39 is what was not written down) | as the attempt goes |
| 28 | any | Delete arrives while another attempt holds it | from the tombstone the other leaves: `applied` by row 26's rule if `erased`, else unfinished; if the other never fenced it, this delete fences it and runs its own attempt, answering as that goes | unchanged by this delete, unless it fences | nothing more, unless it runs its own attempt | — |
| 29 | any | Caller goes away mid-delete | — | the attempt continues to its end | as the attempt goes | as the attempt goes |
| 30 | `fenced`…`settled` | Gateway starts | — (logged) | each tried up to 3 times, 1 s then 2 s apart | as the tries go | slot and release waits handed to the worker; unreadable records and tombstones without a record counted in the log |
| 31a | `fenced` | Retirement while waiting for the stop | unfinished | `fenced` | nothing: not even uploads | left |
| 31b | `fenced` or read | Retirement while waiting for the lease | unfinished | unchanged | as row 9a or 9b, by state | left |
| 32 | `read·session` | Retirement while the agent is asked | unfinished | unchanged | as row 19 | left; the SDK still stops the agent's process, and retirement waits for that |
| 33 | `read·session` | Retirement before the ask | unfinished | unchanged | as row 19 | left |
| 34 | waiting | Retirement | — | unchanged | nothing | the worker ends; nothing new is carried on; a start's finish stops between tries |
| 35 | any | Desktop stop (admission closes; not retirement) | as the attempt goes | as the attempt goes | as the attempt goes | the ask is not ended |
| 36 | waiting | The worker itself panics | — | unchanged | nothing | replaced after one delay with every waiting deletion due, unless retired |
| 37 | past the fence | Any port panics (deletion record's sink, uploads, session storage, summaries, repository) | unfinished | as far as it got before the panic | as far as it got | left |
| 38 | any | The delete fails before its own tombstone write succeeds: the conversation cannot be read, the write fails, the repository panics, or, having waited behind another attempt, it cannot read what that left | that failure's own code (`conversation_storage_unavailable`, `temporarily_unavailable`, …): not known whether the conversation is fenced | as it stands | nothing | — |
| 38b | `fenced` | The history was read, but the tombstone cannot keep what was read | unfinished | `fenced` | uploads let go; history, summary kept; no deletion record | left |
| 39 | `read·session` | Agent answered, but the tombstone cannot record the answer | unfinished | `read·session` | uploads let go; history, summary kept; no deletion record | left; the next attempt asks the agent again |
| 40 | any | Delete refused at the socket: its own pool (8 deletes at once) is full, or the socket already has 16 requests in flight | `temporarily_unavailable`: not known | none written by this delete | nothing | — |

**Each row's tests** (in `crates/nessa-server/tests/conversation/deletion.rs`
unless named otherwise; SDK tests in
`crates/nessa-sdk/tests/infrastructure/acp/contracts/deletion.rs`):

1. `the_agents_own_record_is_asked_to_go_after_the_stop_and_before_the_history`, `delete_stops_a_live_conversation_before_it_records_or_erases_anything`, `the_deletion_record_names_the_conversation_who_deleted_it_and_the_provider_session`, `deleting_on_the_local_stores_erases_what_it_owns_and_leaves_every_audit_record`, `a_person_s_delete_that_finishes_leaves_nothing_waiting`
2. `a_deleted_conversation_refuses_every_command_on_it`
3. `a_deleted_conversation_refuses_every_command_on_it`, `a_create_racing_a_delete_cannot_republish_it`, `a_reopen_racing_a_delete_is_not_recorded_after_the_deletion`, `a_late_reply_cannot_write_back_a_deleted_summary`; `only_the_owner_is_told_a_conversation_was_deleted` (`domain.rs`); `a_deleted_conversation_and_an_unfinished_deletion_have_their_own_codes` (`wire_errors.rs`)
4. `a_delete_after_retirement_has_begun_is_refused_and_fences_nothing`
5. `a_conversation_that_never_opened_names_no_provider_session_and_asks_no_agent`
6. `a_history_moved_aside_is_recorded_as_unknown_and_never_as_no_provider_session`
7. `a_history_that_names_another_session_is_refused_and_nothing_is_erased`
8. `a_delete_whose_agent_stops_after_the_stop_budget_is_finished_once_it_has`, `a_delete_spends_the_stop_and_lease_budgets_it_is_given`, `a_close_that_fails_with_a_deadline_of_its_own_is_not_the_stop_budget` (the budget is its own outcome, never the close's error)
8b. `an_unconfirmed_stop_erases_nothing_and_a_repeat_finishes`; `an_agent_whose_failed_launch_cannot_be_cleaned_up_leaves_the_delete_unfinished` (`application.rs`)
8c. `a_failed_opening_holding_what_it_launched_leaves_the_delete_unfinished` (`application.rs`)
9a. `the_agent_is_not_asked_while_the_history_is_leased_elsewhere`, `a_deletion_left_for_a_held_lease_is_finished_once_it_is_let_go`, `a_history_still_leased_elsewhere_is_left_and_a_repeat_finishes`
9b. `a_lease_held_once_the_answer_is_settled_keeps_only_the_history`
9c. `a_history_that_cannot_be_opened_at_all_is_left_not_carried_on`
10. `a_deletion_left_for_a_held_lease_is_finished_once_it_is_let_go`, `a_lease_wait_is_not_spent_by_slots_freeing`, `a_try_that_turns_to_waiting_on_the_lease_waits_for_its_own_timer`
11. `a_lease_held_past_every_timed_try_is_left_for_the_next_start`
12. `agents_asked_at_once_never_exceed_the_bound_and_the_rest_are_left_unfinished`, `a_delete_turned_away_for_want_of_an_agent_slot_is_finished_once_one_frees`, `an_eraser_answering_capacity_is_a_failure_not_a_slot_wait`, `a_worker_that_panics_is_replaced_with_every_waiting_deletion_due` (a slot wait is due at once)
13. `a_delete_turned_away_for_want_of_an_agent_slot_is_finished_once_one_frees`, `a_slot_freed_by_an_ask_that_panicked_still_wakes_those_waiting`, `a_slot_freed_during_a_try_is_not_lost_when_it_turns_to_waiting_for_one`, `a_slot_wait_that_turns_to_a_release_wait_spends_no_release_try`
14. `the_newest_reason_a_deletion_waits_for_wins`
15. `a_claim_found_held_is_tried_again_once_its_holder_lets_go` (waiting for a slot), `a_release_wait_found_held_elsewhere_spends_no_try_and_finishes_once_let_go` (waiting for a release), `a_person_s_delete_is_never_queued_behind_a_background_try`
16. `what_the_agent_said_it_did_is_recorded_as_it_said_and_the_delete_finishes`, `erasure_is_dispatched_by_agent_and_an_agent_with_no_handler_is_answered_no_handler`; SDK: `claude_is_asked_once_and_a_successful_delete_is_reported_deleted`, `codex_archives_on_delete_so_a_successful_delete_is_reported_archived`, `opencode_claims_nothing_about_a_successful_delete_but_that_it_was_acknowledged`, `an_accepted_delete_never_reads_the_list`
17. `what_the_agent_said_it_did_is_recorded_as_it_said_and_the_delete_finishes`; SDK: `an_agent_that_does_not_offer_deletion_is_not_asked`
18. `a_session_the_agent_deleted_before_the_gateway_could_write_it_down_is_finished_as_not_listed`; SDK: `a_refused_delete_of_a_session_the_agent_does_not_list_settles_as_not_listed`, `a_listing_larger_than_the_protocol_frame_is_still_read`, `the_listing_is_followed_page_by_page`
19. `an_agent_that_cannot_be_asked_or_refuses_leaves_the_deletion_unfinished_and_a_repeat_finishes`, `an_agent_that_keeps_refusing_keeps_our_history_until_its_own_copy_is_gone`; SDK: `a_refused_delete_the_list_cannot_explain_stays_the_refusal`, `an_agent_that_refuses_the_delete_is_a_typed_provider_failure`, `an_agent_that_never_answers_runs_out_of_the_startup_budget_and_is_stopped`, `an_agent_that_cannot_be_launched_is_a_typed_launch_failure`, `an_agent_that_refuses_initialize_is_its_provider_error_and_nothing_is_deleted`
20. `a_slot_freed_by_an_ask_that_panicked_still_wakes_those_waiting`
21. `erasure_is_dispatched_by_agent_and_an_agent_with_no_handler_is_answered_no_handler`, `a_conversation_naming_an_unknown_agent_is_listed_and_deleted_but_not_opened`, `a_conversation_nobody_can_ask_about_takes_no_agent_slot`, `an_agent_not_built_this_run_leaves_the_deletion_unfinished_until_it_is`
22. `an_unavailable_deletion_record_keeps_the_history_but_still_lets_uploads_go`, `a_deletion_that_still_cannot_finish_is_tried_a_bounded_number_of_times_and_reported`; `a_deleted_conversation_and_an_unfinished_deletion_have_their_own_codes` (`wire_errors.rs`)
23. `an_unavailable_record_and_an_unfinished_release_are_both_reported`; `a_deleted_conversation_and_an_unfinished_deletion_have_their_own_codes` (`wire_errors.rs`)
24. `a_summary_that_cannot_be_erased_is_reported_and_a_repeat_erases_it`
25. `a_finished_tombstone_that_cannot_be_written_is_reported_and_a_repeat_writes_it`
26. `the_same_request_from_another_surface_is_not_the_deciding_request`; `the_deciding_request_is_the_same_caller_on_the_same_surface_asking_again`, `the_first_decision_stands_whatever_the_repository_does` (`domain.rs`)
27. `an_unconfirmed_stop_erases_nothing_and_a_repeat_finishes`, `a_history_still_leased_elsewhere_is_left_and_a_repeat_finishes`, `a_summary_that_cannot_be_erased_is_reported_and_a_repeat_erases_it`; `a_tombstone_keeps_the_first_decision_and_reads_its_history_once`, `every_restored_tombstone_state_is_accepted_or_refused_by_the_rule` (`domain.rs`)
28. `a_delete_queued_behind_another_attempt_answers_from_its_tombstone`, `a_repeat_of_the_deciding_request_queued_behind_it_answers_applied`, `concurrent_deletes_of_one_conversation_run_one_after_the_other`, `a_delete_whose_predecessor_never_fenced_fences_it_itself`
29. `a_delete_whose_caller_goes_away_still_finishes`
30. `an_unfinished_deletion_is_finished_and_recorded_when_the_gateway_starts`, `a_deletion_that_still_cannot_finish_is_tried_a_bounded_number_of_times_and_reported`, `a_startup_finish_that_finds_every_slot_taken_is_finished_once_one_frees`; `an_unreadable_record_makes_every_list_incomplete` (`listing.rs`); `a_tombstone_whose_record_was_moved_aside_is_counted_not_listed` (`repository.rs`)
31a, 31b. `a_shutdown_ends_a_delete_waiting_to_stop_or_to_lease_and_it_is_left_unfinished`
32. `a_shutdown_ends_an_agent_that_is_being_asked_and_a_later_start_finishes`, `retirement_waits_for_abandoned_agent_deletions_to_settle`; SDK: `a_deletion_abandoned_mid_exchange_still_stops_its_agent_and_releases_its_home`, `a_binding_settles_once_an_abandoned_deletion_has_released_its_home`
33. `nothing_is_asked_once_retirement_has_begun`
34. `the_worker_carrying_deletions_on_ends_with_retirement`, `nothing_is_carried_on_once_retirement_has_begun`, `a_shutdown_ends_an_agent_that_is_being_asked_and_a_later_start_finishes`
35. `a_desktop_stop_leaves_a_delete_asking_its_agent_alone`
36. `a_worker_that_panics_is_replaced_with_every_waiting_deletion_due`
37. One catch, around every step of `finish_deletion` past the fence, covers all five ports; `a_port_that_panics_after_the_fence_leaves_the_deletion_unfinished` drives it with the deletion record's sink, and `a_slot_freed_by_an_ask_that_panicked_still_wakes_those_waiting` with the eraser's own catch; `a_deleted_conversation_and_an_unfinished_deletion_have_their_own_codes` (`wire_errors.rs`)
38. `a_delete_that_fails_before_its_fence_answers_only_its_own_error`, `a_delete_after_retirement_has_begun_is_refused_and_fences_nothing`
38b. `a_history_read_that_cannot_be_kept_is_the_tombstone_s_failure`
39. `an_answer_that_cannot_be_written_down_is_asked_for_again`
40. `deletes_and_controls_never_take_each_others_place` (`gateway.rs`)

Row 39 asks the agent twice on purpose: what the agent answered cannot be
kept anywhere but the tombstone, and nothing depends on an answer that was not
written down — no deletion record, nothing of ours erased. Asking again is
safe: an agent that deleted the session refuses a second delete of it, and its
list, no longer naming it, settles that as `not_listed` (row 18); one that
archives archives again.

**Agreed with the owner, 2026-09-24** (gate 16: each adds states the simplest
honest behaviour — "not confirmed; the next start or a repeat finishes it" —
would not have):

- *The in-process carry-on worker* (rows 8–15, 34, 36): a person has already
  seen the conversation go and cannot tell it to try again, so a deletion
  waiting only for a free agent slot or for something to let go should not
  wait for a restart.
- *Reading the agent's list after a refusal to settle `not_listed`* (row 18):
  without it, a deletion interrupted after the agent deleted its session would
  be refused by that agent forever, and Nessa's own history would never go.
- *A second delete waiting behind an attempt in progress and answering from
  its tombstone* (row 28): one delete spends at most one attempt, so a
  double-click or a retry never asks the agent twice at once, and still gets
  the true answer.

*Simplified with the owner, 2026-09-24:* a delete's answer promises only
`conversation_erasure_incomplete` and `audit_unavailable` — the conversation
is deleted. Every other error says only that it is not known whether this
delete fenced it; list, or delete again. The gateway no longer tracks, before
and after its lock and after a panic, whether a failing delete had fenced the
conversation: that machinery existed only to promise that other codes meant
nothing was deleted, and each round found a new case it missed (row 38).

## What is kept, and why

Every audit store is kept: conversation creation, files a message pointed at,
execution records, upload records, and the deletion record itself. So is the
ownership record, beside its tombstone. They are the evidence of what happened,
and deletion is one more thing that happened. Deleting a conversation does not
delete the account of it; that is the standard's rule, applied rather than
revisited.

## What Nessa cannot erase

Only what an agent will not erase when asked. Nessa asks every agent that
advertises session deletion, and records exactly what it said it did. Codex's
adapter archives the thread rather than erasing it, so a Codex transcript
remains in Codex's archive; an agent that does not advertise deletion, or has no
handler, keeps its transcript entirely. The deletion record names the provider
session in every case, so what remains elsewhere can be found, and nothing
claims more than the agent confirmed. A person who needs that gone removes it
with the provider's own tools.

A transcript may also remain behind `not_listed`. That outcome means the agent
refused the delete of a session its own list, for the workspace, does not name
— usually one it already deleted — but a list can leave out a session the agent
still keeps: Claude's lists none with no titled prompt, Codex's lists only its
current model provider's threads, and a list for the workspace does not show a
session kept under a workspace since changed. A refusal of such a session, even
a passing one, settles as `not_listed`, and the agent's copy stays. The
deletion record names the provider session, so it can be found.

An agent that keeps refusing the delete of a session it does list keeps the
deletion unfinished, and so keeps Nessa's own history and summary, with no
deletion record written yet: Nessa's record of what was said never goes before
the agent's, since a person who deletes a conversation asks for it gone, not
for Nessa's copy to go while the agent's stays and nothing of Nessa's still
leads to it. The conversation is still deleted — no one can read,
send to, or list it — and every repeated delete and every gateway start asks
again. The operator's remedy is to remove the agent's copy with the agent's
own tools; for an agent that lists its sessions, the next attempt is then
refused as a session it no longer lists, settles as `not_listed`, and
finishes. An agent that keeps no list gives no way to tell that refusal from
any other, so its deletion stays unfinished until it accepts the delete.

What that means in practice was checked on real conversations, not only read
from the adapters. Both agents keep Nessa's sessions in the person's own stores,
beside their own use of those tools. Claude keeps a transcript at
`~/.claude/projects/<workspace>/<session>.jsonl`, and sometimes a `<session>/`
folder beside it; its delete removes both. Codex keeps a rollout under
`~/.codex/sessions/`, a shell snapshot and a writer lock per thread, a line in
`session_index.jsonl`, and rows in its SQLite stores (`state_5`,
`thread_history_1`, `logs_2`); its delete moves the rollout to
`archived_sessions/` and leaves the rest. Nessa does not write into another
tool's private store to finish that job — a database the Codex app may have
open, in a layout that changes with Codex's version — so a Codex conversation
deleted in Nessa is recorded as `archived`, which is what it is. Erasing it
fully would need Codex to offer that, or Nessa to run Codex in a home of its
own; either is its own decision.

## In the panel

The panel does the least that tells the truth. When an archive or delete is
answered, it shows the answer. When the answer is lost, it says the gateway
did not confirm, and shows the list as the gateway reads it next — it does not
infer what happened. Only the latest action speaks.

What the list holds, per window: the rows and archived ids from the newest list
read that succeeded; the rows with an action out (`leaving`); one sentence,
about the latest action; one undo, for the last archive that was answered; and
the conversations known to be deleted (`deletedIds`).

| Event | Rows | Sentence (latest action only) | Undo | Tabs |
| --- | --- | --- | --- | --- |
| Archive, unarchive or delete of X is asked | X leaves the list until answered | cleared | cleared, unless this is the undo of X; not shown while it is out, so it cannot be asked twice | — |
| Archive answered, applied | X archived | none | offered for X | kept |
| Unarchive answered, applied | X back once the list is read again, or at once as a tab open on it | none | withdrawn | kept |
| Archive or unarchive answered, not applied | as the next read says | "The gateway changed nothing for “X”." | none | kept |
| Delete answered (acknowledged, or refused with `conversation_erasure_incomplete` or `audit_unavailable`, which a delete answers only once the conversation is deleted) | X gone for good (`deletedIds`) | none, or "“X” was deleted." and what is left to finish | withdrawn if for X, whichever action is latest | let go |
| Refused with a reason | X back | "Could not … “X”: reason." | an undo of X stays on offer unless the reason is permanent (deleted, not found) | kept |
| Refused as `conversation_deleted` | X gone for good (`deletedIds`) | as above | withdrawn if for X | kept, each told when it reads |
| Answer lost, or its outcome unknown — including a delete answered with any code but the two above, since the gateway promises only that those two mean deleted | X back until the next read says otherwise | "The gateway did not confirm whether “X” was …. The list shows where it stands." | a lost undo stays on offer; a lost archive offers none | kept |
| Not connected | X back | "Could not … “X”: Nessa is not connected to the gateway, so nothing was done." | as refused | kept |
| An older action answers after a newer one was asked | its own row settles; a delete it confirms still lets go of tabs and adds `deletedIds` | unchanged | unchanged, except that a delete of X withdraws an undo of X | as its row |
| Any answer | the list is read again | — | — | — |
| A list read succeeds | replaced, newest read wins | unchanged | unchanged | — |
| A list read fails | kept | "The list could not be refreshed, so it may be out of date." beside it | unchanged | — |
| The Messages tab is left | — | cleared; an action still out says what became of it when it answers | withdrawn | — |

The panel shows no archived conversations, so the undo is the way back to one;
talking in it again from a tab still open on it also brings it back. A lost
delete keeps the tabs: a tab onto a conversation that was in fact deleted, here
or on another surface, is told so the next time it reads — "Conversation
deleted", with nothing to retry — so a draft written in it is still there to
copy. Until then its row can still show, as any tab the list has not caught up
with does. An archived conversation leaves the list even while a tab holds it
open. The list's `complete` flag is the gateway's; the panel does not read
anything into a conversation's absence from the list.

## Not decided here

A Recently Deleted area with delayed purge, bulk delete, an archived view, and
erasing what an agent's own delete leaves behind (Codex's only archives). Each would be its own decision.
