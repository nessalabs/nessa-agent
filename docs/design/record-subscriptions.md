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
second and `conversation.read` every 250 ms for the conversation on screen. A
change waited up to a poll interval, an idle window still asked, and a missed
change was mended only by the next poll.

## What a subscription is

A subscription is "read now, then read again whenever the committed records or
the live overlay could have changed, and send what changed". It is not a stream
of records. Every frame is the same bounded replacement a one-shot read
returns, built by the same fold:

- `conversation.subscribe {conversationId, after?}` sends `conversation.view`
  frames: `{subscriptionId, cursor, view}`. `view` is exactly what
  `conversation.read` returns. `cursor` is `{incarnation, position}`, the
  committed position the fold had reached when the view was read.
- `conversation.subscribeList {archived?}` sends `conversation.listed` frames:
  `{subscriptionId, list}`, where `list` is exactly what `conversation.list`
  returns.
- `conversation.unsubscribe {subscriptionId}` stops one.
- `conversation.subscriptionEnded {subscriptionId, reason, code?, lastDelivered?}`
  is the one terminal frame. `reason` is `lagging`, `refused`, `source_closed`
  or `too_large`; `code` says why a `refused` one was refused;
  `lastDelivered` is the cursor of the last view frame written, so a lagging
  subscriber resumes from it.

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
is the whole no-gap argument (ADR 0009, "notification rule"). Commits that land
while a frame waits to be written collapse into one dirty bit, so a busy
conversation is never queued up behind a slow client.

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

### Lagging and slow clients

The task offers its frame to the connection's writer and waits. If the writer
has not taken it within `deliveryTimeoutMs` (published in
`x-subscriptionLimits`), the task withdraws it and ends the subscription with
`lagging` and `lastDelivered`; the socket stays open. A write that has started
and stalls past the socket's write timeout closes the socket, as every other
frame does. The terminal frame itself must be written by its deadline or the
socket closes, as a watch's terminal notice does.

Nothing a producer does waits on a socket: a commit flips a dirty bit
(SDK `RecordChanges::publish`), a live change bumps a `tokio::sync::watch`
value, and reads happen in the subscription's own task under the server-wide
`requests` capacity. One stalled socket therefore delays only its own frames
(row S12).

### Where batches are authorized

`product/subscription/target.rs::authorize_batch` is the one place a batch is
admitted. Slice G (read grants) adds its per-conversation check there; nothing
else decides whether a frame may be read.

### Limits

`x-subscriptionLimits` publishes `conversationTargets` (8) and `listTargets`
(1) per connection, and `deliveryTimeoutMs` (10000). The SDK and catalogue
watch registries keep their own producer bounds (64 each); a full one refuses
`subscription_capacity`. A list frame larger than the 64 KiB frame bound is
cut, newest first, and marked `complete: false` (row L3): the client walks
`conversation.observe` for the rest, as it does for any incomplete list.

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
| S9 | A wake that changes nothing the client sees | No frame | `a_wake_that_changes_nothing_sends_nothing` |
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
| L1 | List subscription; a catalogue change | A new list frame | `a_list_subscription_follows_the_catalogue` |
| L2 | A turn starts or ends without a summary change | A new list frame with `running` changed | `a_turn_without_a_summary_change_updates_the_list` |
| L3 | A list larger than one frame | Cut newest first, `complete: false` | `a_list_too_large_for_one_frame_is_cut_and_marked_incomplete` (unit) |
| L4 | A stream of commits under a list subscription | List frames no closer than `LIST_REREAD_FLOOR` | `list_frames_come_no_closer_than_the_reread_floor` |

Desktop rows (`src/desktop/workspace/adapters/gateway/gateway-source.test.ts`;
the test names begin with the row id). The adapter subscribes only: it sends
no `conversation.read` or `conversation.list`, and the `GatewayClient` type it
asks of the client does not offer them.

| Row | State and input | Result |
|---|---|---|
| D1 | First listener | One list subscription; nothing asked on a timer |
| D2 | A followed conversation's frames | A frame with a new gateway revision is the next transcript; the same revision is the same transcript |
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
| D17 | The retry clock | Runs only while someone listens and something is not subscribed; stops when the last listener leaves |
| D18 | An incomplete frame naming the same conversations as the last one walked | No walk; one naming others walks again |
| D19 | `after` refused `cursor_ahead` | Opened once more without `after` |
| D20 | `transcript` of a conversation followed | The view held; no second subscription |
| D21 | An answer in a conversation let go past the limit | Followed again before the answer is sent |
| D22 | `dispose` | Every subscription closed; nothing applies after |

The rows of #248 and #419 that do not depend on a timer read keep their ids
(connection C, writes W, refusals F, connect rules S); the poll-cadence rows
(round counts, read pacing, foreground polling of #532) went with the poller.
`W6` gained a case: a list frame that arrives while an archive is on its way
may have been read before it, and does not list the archived session again.
