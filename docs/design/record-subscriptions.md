# Record subscriptions — issue 702, slice E

Status: this branch. Part of [#252](https://github.com/nessalabs/nessa-agent/issues/252)
(slice E of [ADR 252](../adr/todo/252-runtime-roles-and-execution-leases.md)).
The rules come from [ADR 0009](../adr/todo/0009-reusable-event-stream-crate.md)
(a live notice tells a reader to check the store) and
[ADR 0011](../adr/todo/0011-nessa-session-protocol-and-authorities.md) phase A
(one batch at a time per subscription, access checked again for each batch).
[Committed change watches](committed-change-watches.md) own the payloadless
notices this builds on; the [transcript fold](transcript-fold.md) owns the one
`Projection`.

## The problem

The desktop learned about changes by asking again: `conversation.list` every
second and `conversation.read` every 250 ms for the conversation on screen, in
both the workspace and the conversation panel. A
change waited up to a poll interval, an idle window still asked, and a missed
change was mended only by the next poll.

## What a subscription is

A subscription is "read now, then read again whenever the committed records or
the live overlay could have changed, and send what changed". It is not a stream
of records. Every frame is the same bounded replacement a one-shot read
returns, built by the same fold:

- `conversation.subscribe {conversationId, after?}` sends `conversation.view`
  frames: `{subscriptionId, cursor, view}`. `view` is what
  `conversation.read` returns, read through the same function, except that
  following a conversation never opens its agent (below). `cursor` is
  `{incarnation, position}`, the committed position the fold had reached when
  the view was read.
- `conversation.subscribeList {archived?}` sends `conversation.listed` frames:
  `{subscriptionId, list}`, where `list` is exactly what `conversation.list`
  returns.
- `conversation.unsubscribe {subscriptionId}` stops one.
- `conversation.subscriptionEnded {subscriptionId, reason, code?, lastDelivered?}`
  is the one terminal frame. `reason` is `lagging`, `refused`, `source_closed`
  or `too_large`; `code` says why a `refused` one was refused;
  `lastDelivered` is where a view subscription resumes from: the cursor of
  the last view frame written or, when none was, the `after` it was opened
  with (absent when neither exists), so a lagging subscriber resumes from it.

Why views and not record batches: the desktop is TypeScript and must not keep
a second fold (rule "one fold" in the transcript-fold design). Devices that do
fold records themselves already have a replay-to-live path on records: verified
`conversation.recordsHead`/`recordsPage` reads driven by
`conversation.watchRecords` notices. That path is unchanged.

## How it works

One task per subscription, owned by the connection
(`product/subscription/target.rs`):

1. Register the wake sources **before** the first read: the SDK record watch
   for the conversation (or, for a list, the owner's catalogue watch and the
   SDK watch on every commit), and the service's live-change signal.
2. Check access for this batch (`authorize_batch`: current session, the
   method's Cedar grant, browser presence) and read through the service, which
   checks ownership again.
3. Send the frame if what the client would see changed. Wait until the
   connection's writer has written it before reading again: one frame in
   flight per subscription.
4. If the read left the transcript `stale` (a long history, read
   `COMMITTED_READ_FRAMES` at a time) and the cursor moved since the last
   read, read again at once. Otherwise wait for a wake. Not `partial`: that
   is an unfinished fact at a confirmed head, which reading again does not
   change, so it would read in a loop.
5. A read refused for want of a storage read slot (`Busy`, `ReadCapacity`) is
   tried again a few times with a doubling wait before the subscription is
   refused (row S23): one commit wakes every subscription at once, and the
   SDK runs a few reads at a time. A list waits `LIST_REREAD_FLOOR` (250 ms)
   after a wake before it reads, so a reply saved every 100 ms costs a few
   list reads a second, not one per commit (row L4). A view never waits.

Because registration comes before the read, a commit that lands during or
after the read leaves the watch dirty, and the next wait returns at once. That
is the whole no-gap argument (ADR 0009, "notification rule"; rows S7, S8).
Commits that land while a frame waits to be written collapse into one dirty
bit, so a busy conversation is never queued up behind a slow client (rows S8,
S12).

### A follower opens nothing

`conversation.read` opens a conversation whose agent is not live, so its view
can say what the agent can do. A subscription reads with
`ReadOpening::LiveOnly` instead: when no agent is live or opening, the view is
the committed records folded with no agent (`read_unopened`), read-only, its
lifecycle `absent`, nothing awaiting an answer. Otherwise a close or a desktop
stop, which publishes the slot let go, would wake every follower into a read
that starts the agent again at once (row S26). A send opens the agent, and its
publish wakes the view. A fresh fold numbers its revision anew, so the
comparison that keeps an unchanged view from being sent leaves the revision
out (row S9). Nor does a follower recover an unfinished approval-mode change,
which stops and starts the agent: it shows the change pending and leaves
recovery to `conversation.read` or a send (row S27).

### Live overlay

A view is the committed fold plus live facts that are not records: the
agent's lifecycle and capabilities, which permission asks the agent still
waits on, the title (from the summary store), the approval-mode change in
progress, and MCP App reviews. The conversation service owns
`LiveChanges`, one signal per conversation somebody follows, and publishes it
where those facts change: an agent
opening finished or failed, a slot let go, a summary written, a mode change
started or finished, and every change to an App's pending reviews. A
subscription re-reads on it; an unchanged view is not sent (row S9). The
signal is keyed by conversation, so a change to one wakes only its own
subscribers; an entry goes once nobody follows it.

### The cursor

`after` is the cursor of the last view the client applied. The server never
sends a frame behind it in the same incarnation (row S5), so a reconnect or a
resume after `lagging` does not move the client backwards while a restarted
gateway replays a long history. A cursor from another incarnation means the
store was reset: the current view is sent as a replacement (row S4). A cursor
ahead of the history the store holds is refused with `cursor_ahead` (row S3).
Every subscription sends at least one frame at or past `after`, because the
overlay may have changed even if no record did.

### One open per target, even when given up

The gateway allows one live subscription per target on a connection and
refuses a second (`subscription_duplicate`). A follower that stops before its
open is answered has no handle to close, so following again could reach the
gateway while the first is still live. The rule that prevents it lives once,
in the client (`packages/nessa-client/src/presentation/subscription-gate.ts`,
used by `subscriptions.view` and `.list`): an open whose `signal` is aborted
before its answer is closed once answered, and the next open of that target
is sent only after the close. The desktop source (D23) and the conversation
panel (P4) only abort; the desktop's test gateway uses the same gate. The
gateway admits an unsubscribe inline, so a close sent before a subscribe
lands before it.

### A frame that comes with its answer

The gateway writes a subscription's first frame right after its reply, and a
transport may hand both over in one turn (a socket that reads several
messages at once), before the open has registered the identity the reply
names. The client keeps frames of no live subscription while an open is on its
way, the last of each identity and its end; the open that is answered with
that identity takes them, in order, as it registers it (row A1). Nothing is
kept once no open is on its way, so a frame of a subscription already closed
is still dropped.

### Lagging and slow clients

The task offers its frame to the connection's writer and waits. If the writer
has not taken it within `deliveryTimeoutMs` (published in
`x-subscriptionLimits`), the task withdraws it and ends the subscription with
`lagging` and `lastDelivered`; the socket stays open. The writer takes the
connection's own replies and notices before subscription frames, so a socket
kept busy with its own traffic for the whole timeout can end a subscription
`lagging` too; the client resumes it the same way. A write that has started
and stalls past the socket's write timeout closes the socket, as every other
frame does. The terminal frame itself must be written by its deadline or the
socket closes, as a watch's terminal notice does; so must the subscribe reply,
by the request's reply deadline, as a watch's reply (row S31): written after
the client stopped waiting, it would name a live subscription the client never
learns, and every subscribe again would be refused `subscription_duplicate`.

Nothing a producer does waits on a socket: a commit flips a dirty bit
(SDK `RecordChanges::publish`), a live change bumps a `tokio::sync::watch`
value, and reads happen in the subscription's own task under the server-wide
`requests` capacity. One stalled socket therefore delays only its own frames
(row S12).

### Where batches are authorized

`product/subscription/target.rs::authorize_batch` is the one place a batch is
admitted. A subscription's task asks it before it registers any wake source,
so a session that may not follow takes nothing from the shared watch pools,
and again before every read, once the batch holds its `requests` permit: a
request holds its capacity before it is admitted (`dispatch` does the same), so
nothing revoked while a batch waits for a permit is read (row S30). An unsubscribe is admitted by the socket before
it changes anything (row S29). Both use the same admission as `dispatch`
(`admit_now`). Both targets ask the read grant there too: a view batch for its
conversation, and a list batch, which is refused to a paired device
([read grants](read-grants.md)); nothing else decides whether a frame may be read.

### Limits

`x-subscriptionLimits` publishes `conversationTargets` (8) and `listTargets`
(two: active and archived) per connection, and `deliveryTimeoutMs` (10000). The SDK and catalogue
watch registries keep their own producer bounds (64 each); a full one refuses
`subscription_capacity`. A list frame larger than the 64 KiB frame bound is
cut, newest first, and marked `complete: false` (row L3): the client walks
`conversation.observe` for the rest, as it does for any incomplete list. A
list the service bounds (`MAX_LISTED_CONVERSATIONS`) is incomplete the same
way. Either way the rows a frame carries cannot show a change to a row it
left out, so an incomplete list is sent again after any catalogue change,
though the rows are the same, and the client walks every incomplete frame
(rows L5, D18). A commit alone (a turn's `running`) sends a frame only when
the rows change. Before a list reads, it takes every notice already waiting,
so a commit that dirtied both the records and the catalogue is one read and
at most one frame; and the client applies only the newest of the frames that
queue behind a walk, so frames during a walk cost one more walk, not one each.

## State and order table

Each row has one test. Server tests are in
`crates/nessa-server/tests/product/socket/subscriptions.rs` unless named
otherwise.

| Row | State and input | Result | Test |
|---|---|---|---|
| S1 | Subscribe to a conversation | Wake sources registered, first read, then the reply, then the first frame | `subscribe_replies_after_the_first_read_then_sends_the_view` |
| S2 | Subscribe; the first read is refused | The reply carries the read's code; nothing is registered | `a_refused_first_read_refuses_the_subscription` |
| S3 | `after` names a position past the store's head | Refused `cursor_ahead` | `a_cursor_ahead_of_history_is_refused` |
| S4 | `after` from another incarnation | The current view is sent | `a_cursor_from_another_incarnation_gets_the_current_view` |
| S5 | A long history and an `after` inside it | Replayed in bounded reads, one frame in flight; no frame behind `after` | `a_long_history_replays_in_bounded_frames_and_never_behind_the_cursor` |
| S6 | A commit after the first frame | One new frame | `a_commit_after_subscribe_is_delivered` |
| S7 | Commits racing the subscription's reads | The last frame shows the last commit; cursors only move forward | `replay_then_live_misses_no_commit_under_concurrent_writes` |
| S8 | Many commits while a frame waits for the writer | One next frame, with the latest state | `commits_while_a_frame_waits_collapse_into_one_frame` |
| S9 | A wake that changes nothing the client sees (the revision alone may differ) | No frame | `a_wake_that_changes_nothing_sends_nothing`; `views_that_differ_only_in_revision_have_one_key` (unit) |
| S10 | A frame not taken by the writer within `deliveryTimeoutMs` | Ended `lagging` with `lastDelivered`; the socket stays | `a_frame_not_taken_in_time_ends_the_subscription_as_lagging` |
| S11 | Resubscribe after `lagging` from `lastDelivered` | Frames continue; the last equals a cold read | `a_lagging_subscriber_resumes_from_its_cursor` |
| S12 | One socket's writer stalled | Commits finish and another socket's frames arrive | `one_stalled_socket_delays_no_commit_and_no_other_subscriber` |
| S13 | Live frames versus a cold full replay | The same view | `live_frames_and_a_cold_full_replay_build_the_same_view` |
| S14 | The grant is revoked between batches | Ended `refused` with `forbidden` before the next read | `a_revoked_grant_ends_the_subscription_before_the_next_batch` |
| S15 | The conversation is deleted | Ended `refused` with `conversation_deleted` | `deleting_the_conversation_ends_its_subscription` |
| S16 | Unsubscribe | No frame of it after the reply | `no_frame_follows_an_unsubscribe_reply` |
| S17 | Past the per-connection limit | Refused `subscription_capacity` | `subscriptions_past_the_published_limit_are_refused` |
| S18 | A second subscription to the same target on one connection | Refused `subscription_duplicate` | `a_duplicate_subscription_is_refused` |
| S19 | The record source closes | Ended `source_closed` | `a_closed_source_ends_the_subscription` (unit, `product/subscription/target.rs`) |
| S20 | A live overlay change with no commit (the approval mode changed) | One new frame; the cursor does not move | `an_overlay_change_without_a_commit_is_delivered` |
| S21 | The socket closes | Every task ends and every registration is dropped | `closing_the_socket_drops_every_subscription` |
| S22 | A frame larger than the frame bound | Ended `too_large` | `an_oversized_view_ends_the_subscription_as_too_large` (unit, `product/subscription/target.rs`) |
| S23 | A read refused for want of a storage read slot | Tried again with a doubling wait, then refused | `a_read_without_a_storage_slot_is_tried_again_then_refused` (unit, `product/subscription/target.rs`) |
| S24 | The writer took a frame and is still writing it when its delivery deadline passes | Written; not ended `lagging` | `a_frame_taken_as_its_deadline_passes_is_written_and_ends_nothing` (unit, `product/subscription/target.rs`) |
| S25 | The first read has not finished by the subscribe request's passive read deadline (`readTimeoutMs`), so the rest of its delivery budget is left for the reply (S31) | Refused `unavailable`; its wake sources go with it | `a_first_read_past_the_reply_deadline_is_refused_unavailable` (unit, `product/subscription/target.rs`) |
| S26 | A followed conversation is closed or stopped (its agent let go) | One frame: the same history, read-only, lifecycle `absent`; the agent is not opened again until a send opens it | `a_closed_conversation_is_shown_let_go_and_not_opened_again` |
| S27 | A followed conversation has an unfinished approval-mode change | The view shows the change pending; recovery is not run and the agent is not opened | `a_pending_mode_change_is_shown_not_recovered_by_a_follower` |
| S28 | The agent's opening fails while a follower waits on it (or failed and holds its slot) | The follower is shown the committed history, read-only; `conversation.read` and a send report the failure | `a_failed_opening_holding_its_slot_is_read_by_a_follower_as_its_history` (`tests/conversation/desktop_stop.rs`) |
| S29 | A subscribe or unsubscribe the session may not make (grant, presence, or no longer current) | Refused with the admission's code; a subscribe takes no watch from the shared pools first | `a_forbidden_subscribe_is_refused_before_it_takes_a_watch`, `a_forbidden_unsubscribe_is_refused` |
| S30 | Every request permit taken when a batch is woken; the grant is revoked while it waits for one | Ended `refused` with `forbidden`; no frame is read under the old grant, because a batch is admitted only once it holds its capacity, as `dispatch` admits a request | `a_grant_revoked_while_a_batch_waits_for_capacity_ends_it_before_the_read` |
| S31 | The subscribe reply is not written by the request's reply deadline (the writer held behind other traffic) | The socket closes, as for a watch's reply; the client reconnects and subscribes again, never left with a live subscription it cannot name | `a_subscribe_reply_not_written_by_its_deadline_closes_the_socket`; `the_reply_and_the_last_frame_carry_a_deadline_through_their_write` (unit, `product/subscription/delivery.rs`) |
| L1 | List subscription; a catalogue change | A new list frame | `a_list_subscription_follows_the_catalogue` |
| L2 | A turn starts or ends without a summary change | A new list frame with `running` changed | `a_turn_without_a_summary_change_updates_the_list` |
| L3 | A list larger than one frame | Cut newest first, `complete: false` | `a_list_too_large_for_one_frame_is_cut_and_marked_incomplete` (unit) |
| L4 | A stream of commits under a list subscription | List frames no closer than `LIST_REREAD_FLOOR` | `list_frames_come_no_closer_than_the_reread_floor` |
| L5 | An incomplete list; a row the frame left out changes: by the catalogue (archived, deleted), or by a commit alone (its `running`, within the service's bound), so the rows it carries are the same | A frame all the same, so the client walks the catalogue (D18); a commit that changes no row the service read sends none; notices already waiting when a list reads are taken by that read | `a_change_the_incomplete_list_leaves_out_is_still_sent`; `a_commit_to_a_row_the_frame_cut_is_still_a_change`, `a_list_takes_every_waiting_notice_before_it_reads` (unit, `product/subscription/target.rs`) |

Desktop rows (`src/desktop/workspace/adapters/gateway/gateway-source.test.ts`;
the test names begin with the row id). The adapter subscribes only: it sends
no `conversation.read` or `conversation.list`, and the `GatewayClient` type it
asks of the client does not offer them.

| Row | State and input | Result |
|---|---|---|
| D1 | First listener | One list subscription; nothing asked on a timer |
| D2 | A followed conversation's frames | A frame a person would see differently is the next transcript, whatever its revision (a title, a permission ask, an app's review change without it); one that differs only in its revision is the same transcript |
| D3 | Ended `lagging` (view or list) | Opened again at once, a view from the last cursor applied; no gap |
| D4 | Ended `refused` as not found or deleted | The session is taken out; a deletion is also forgotten by the window's MCP Apps |
| D5 | The connection is lost, then back | Every subscription ends `disconnected` and nothing more; back, one resync and each opened again from its cursor |
| D6 | An incomplete list frame | `conversation.observe` walked in the list's turn until the pass finishes |
| D7 | Opening past `subscriptionLimits.conversationTargets` | The least recently opened (by `transcript`, `send` or `appCall`; a frame does not count) is let go, its last frame dropped, its summary said from its row; opened again it is followed again |
| D8 | A frame behind the cursor applied, same incarnation | Not applied; a frame from another incarnation is |
| D9 | The list subscription ends, not lagging | A gap: opened again on the retry clock, never at once; its first frame resyncs once |
| D10 | Ended `too_large`, or a frame the client could not read (`invalid_frame`) | Let go with a warning: no retry, no gap; asked again it follows again |
| D11 | An MCP App's call in a conversation | That conversation followed (not one taken out); it stays followed after the call |
| D12 | An answer taken | No read is sent; the next frame carries the conversation |
| D13 | A subscription answered after it was let go (the session taken out, the call out of time) | Closed unused; its frames apply nothing |
| D14 | `transcript`, `appCall` and the retry clock at once | One subscription |
| D15 | A subscription that could not open | A gap, opened again on the retry clock; the next list frame resyncs |
| D16 | Ended `source_closed` | Opened again on the retry clock from its cursor |
| D17 | The retry clock | Runs only while someone listens and something is not subscribed or a list walk is owed (D18); stops when the last listener leaves |
| D18 | An incomplete frame, whatever conversations it names; frames that come while a walk is on its way | Walked, so a row it left out that went is taken out (L5 sends one for that); of the frames queued behind a walk only the newest is applied, and walks once; a walk that fails leaves the list unapplied and a gap, and the frame is walked again on the retry clock, or at once by the next index, unless a newer frame walks first |
| D19 | `after` refused `cursor_ahead` | Opened once more without `after` |
| D20 | `transcript` of a conversation followed | The view held; no second subscription |
| D21 | An answer in a conversation let go past the limit | Followed again before the answer is sent |
| D22 | `dispose` | Every subscription closed; nothing applies after |
| D23 | A conversation let go past the limit while its open is on its way, then opened again | The open is given up (its `signal`); subscribed again only once it has been answered and its close finished, so the gateway does not refuse it `subscription_duplicate` |
| D24 | The last listener leaves while the list or a conversation is opening, and one comes back before it is answered (StrictMode) | Every open on its way is given up as in D23, list and conversation alike; both are opened again once the old opens are closed, and a transcript waiting for a frame is answered by the new one; an index call that awaited the old list open is refused `unavailable` and asked again |
| D25 | A conversation let go past the limit while its open is on its way, and the one that took its place | The new one is subscribed only once the old open is answered and closed: the gateway counts an open on its way against the limit, and would refuse it `subscription_capacity` |

Conversation panel rows (`src/conversation/adapters/gateway/effects.test.ts`,
`adapters/store/slice.test.ts` and `adapters/store/follow-replacement.test.ts`; the test names begin with the row id).
The tab on screen follows its conversation through `ConversationEffects.follow`;
the panel's poller is gone, and nothing it does asks on a timer while a view
subscription is open.

| Row | State and input | Result |
|---|---|---|
| P1 | Ended `lagging` | Subscribed again at once from the last frame's cursor; nothing said |
| P2 | `after` refused `cursor_ahead` | Subscribed once more without `after` |
| P3 | Refused, or ended for any reason but `lagging` | The tab keeps its view and shows the read failure; subscribed again after `FOLLOW_RETRY_MS` (the old idle pace), except for a deleted conversation |
| P4 | A follow answered or framed after it was stopped or replaced; followed again while its open is on its way | Closed unused; its frames apply nothing (`readRequest`); the next follow subscribes only after that close |
| P5 | A command answered (send, control, stop) | A followed tab is followed again, so its next view is read after the answer; a tab not on screen is read once and left unfollowed |
| P6 | A frame identical to the last one this follow applied | Not applied again, so what is on screen keeps its references; any other frame applies, whatever its revision |
| P7 | The tab switched, closed or unmounted | Its follow stopped; nothing it says applies after |
| P8 | A tab followed while its after-command single read is on its way (switched to before its first frame) | The single read is stopped by the follow that replaces it, and settles, so the command it was reading for finishes; the store is the one place a tab's follow is replaced, a single read included, and the effects stop a follow only by its own function |

### Panel Messages list — issue 722

The Messages list follows the two explicit catalogue classifications through
`ConversationEffects.followList(archived, follower)`. The product manifest owns
`listTargets` (two): one active and one archived list on the existing connection.
The panel does not infer archive status from a row missing from the active list.
The history slice owns one paired follow per dependency scope, closes it when the
Messages list leaves, and gives every replacement a fresh request identity.
The client subscription gate owns abort-before-answer cleanup for each target.

The first publication waits for both lists. Later each half replaces its own
facts while retaining the other half. Those halves are separate bounded reads,
not an atomic catalogue snapshot: an archive may temporarily occur in both
halves, or neither. Explicit archived evidence excludes a held tab; omitted rows
are not a tombstone. Incompleteness stays visible. An incomplete archived frame
retains known archived identities unless an explicit active row says otherwise;
identities explicitly named in the current archived frame remain excluded.
A failed half retains its last facts and a stale-list notice, even while the
other half updates. Neither success in the other half nor omission clears it.
Commands restart the pair after their answer (including a lost answer), so frames
read before an archive, undo or delete cannot replace their local evidence.
One-shot list reads remain available while no Messages follow is held; their late
answers cannot replace a held follow. No active subscription asks on a timer.

Panel list rows (`adapters/store/history-follow.test.ts`,
`adapters/gateway/list-follow.test.ts`, and `ui/conversation-list.test.ts`).

| Row | State and input | Result |
|---|---|---|
| PL1 | Only one initial list has arrived, in either order | No half-list publication; both initial frames publish rows and explicit archived identities |
| PL2 | An external create, archive, unarchive or delete changes one half | Replace only that half, retaining the other; archived identities are explicit, not inferred from missing active rows |
| PL3 | One half fails, before or after its first frame | Retain published facts; show its typed stale failure; a successful other half does not clear it; a frame from the failed half clears that failure |
| PL4 | The Messages list is mounted twice, replaced or leaves while a subscribe answer is pending | One paired owner; stop aborts each target; the client gate closes late answers; stopped frames/errors apply nothing |
| PL5 | A one-shot list began before a follow, or answers while one is held | Its answer/failure cannot replace the follow; a later one-shot after unmount is allowed |
| PL6 | Archive, undo or delete answers, or its answer is lost | The held pair is restarted with a fresh identity; old frames cannot erase local archive/delete evidence; its original UI cleanup still closes the restarted pair |
| PL7 | Gateway reconnect or session availability changes | Replace the pair; the prior scope's cleanup and callbacks cannot stop or update the new one |
| PL8 | A list ends lagging | Subscribe again at once without polling; any other end/refusal reports typed failure and waits the existing bounded retry pace |
| PL9 | An archived list is incomplete | Keep explicit known archived identities not contradicted by an active row; current explicit archived rows stay excluded; completeness remains false |
| PL10 | One-shot reads or a failed half coexist with local archive/delete/undo evidence | The list projection retains leaving/deleted identities and action ordering; recovery publishes only under the current pair identity |

Client rows (`packages/nessa-client/src/presentation/subscription-api.test.ts`;
the test names begin with the row id).

| Row | State and input | Result |
|---|---|---|
| A1 | The subscribe's answer and the subscription's first frames (or its end) in one turn, before the open has registered it | The open hands them over as it registers: the latest frame, then the end; a frame of an identity no open claims is dropped |

The rows of #248 and #419 that do not depend on a timer read keep their ids
(connection C, writes W, refusals F, connect rules S); the poll-cadence rows
(round counts, read pacing, foreground polling of #532) went with the poller.
`W6` gained a case: a list frame that arrives while an archive is on its way
may have been read before it, and does not list the archived session again.
