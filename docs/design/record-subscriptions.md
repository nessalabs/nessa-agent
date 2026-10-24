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

The agent catalog is followed the same way (issue 706, part I-3; row I10 in
[peer gateways](auth/peer-gateways.md#the-agent-catalogue-follows-the-connected-environments-row-i10)):

- `agents.subscribe {}` sends `agents.listed` frames: `{subscriptionId,
  catalog}`, where `catalog` is exactly what `agents.list` returns, read
  again each time a paired environment connects, disconnects or is revoked,
  and after each native hooks command. Its wake sources are
  `PeerEnvironments::connections` and `NativeHooksPreferences::changes`; it
  reads no conversation and registers no conversation source.
- `agents.unsubscribe {subscriptionId}` stops it. It asks `server.read`, the
  grant `agents.subscribe` and `agents.list` ask, where
  `conversation.unsubscribe` asks `conversation.write`; so each stops only
  what the subscribe beside it made, and answers `unknown_subscription` for
  the other's.
- It ends with the same `conversation.subscriptionEnded`, with no
  `lastDelivered`.

Why views and not record batches: the desktop is TypeScript and must not keep
a second fold (rule "one fold" in the transcript-fold design). A linked device
follows one conversation with `conversation.subscribe` on a new pinned
session to the same gateway as its checkpoint read. The two calls do not
share a connection. The checkpoint read still copies records
(`conversation.recordsHead` / `recordsPage`). Subscriptions are the only live
read path: the product-socket watch (`conversation.watchRecords`,
`conversation.watchCatalogue`, `conversation.unwatch`) was deleted in #257. The
example client's bounded `follow` keeps its private record cache fresh by
running a checkpoint pass whenever a view's cursor moves, on the same
connection as that pass ([read-only sync example](read-only-sync-example.md#following-a-conversation)).
`conversation.list`, `.observe` and `.subscribeList` use a device's grant-scoped
catalogue. The bounded rows and scoped head of a list come from one metadata
snapshot; an incomplete frame compares that head, so an unrelated wake sends
nothing (L6–L13). A peer gateway retains its existing catalogue read path.
The desktop shows the
view's own `truncated` and `transcriptState` (`history` on its transcript).
It does not page records to reconstruct older text, and a turn the view left
out does not hide the turns the view did draw.

## How it works

### Stress capacity observation

The follower's capacity probe counts only one unchanged set of physical follows.
The probe retires its temporary views in every outcome; a later loop retries an
interrupted observation without changing the connection's published limit.

| Arrival during the probe | Observation | Regression |
| --- | --- | --- |
| Original view and event follows remain live; the extra view is refused | Count the expected capacity refusal | `a stable full connection proves its capacity` |
| An original event ends, is removed, or is replaced while a view opens | End this observation because real room changed | `an original event ending invalidates the probe`; `replacing an original event invalidates the probe` |
| The original main view ends while a view opens | End this observation | `the original main view ending invalidates the probe` |
| A temporary view ends before the final response | End this observation; close each acquired handle | `an acquired view ending invalidates the probe` |
| Every captured source remains live and the extra view is accepted | Record a capacity failure and close the acquired views | `a stable connection accepting an extra view fails` |

One task per subscription, owned by the connection
(`product/subscription/target.rs`):

1. Register the wake sources **before** the first read: the SDK record watch
   for the conversation (or, for a list, the owner's catalogue watch and the
   SDK watch on every commit), and the service's live-change signal.
2. Check access for this batch (`authorize_batch`: current session, the
   method's Cedar grant, browser presence) and read through the service, which
   checks ownership again. A paired device's view subscription is admitted on
   `conversation.read`, the grant pairing mints, or on `conversation.write`
   when that credential still holds it; which conversation it may follow is
   the share check (`admit_conversation`). A device's list uses that same method
   admission, then `grant.list` for the catalogue and `grant.read` for each row.
3. Send the frame if what the client would see changed. Wait until the
   connection's writer has written it before reading again: one frame in
   flight per subscription.
4. If the read left the transcript `stale` (a long history, read
   `COMMITTED_READ_FRAMES` at a time) and the cursor moved since the last
   read, read again at once. Otherwise wait for a wake. Not `partial`: that
   is an unfinished fact at a confirmed head, which reading again does not
   change, so it would read in a loop.
5. A storage read waits for a free read thread rather than failing. A read
   still refused for want of room (`Busy`, or `ReadCapacity` when the view
   cache holds only pinned entries) is tried again a few times with a doubling wait before the subscription is
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
publish wakes the view. A fresh fold numbers its revision anew, and every
read is stamped with the gateway's time (`serverNowMs`), so the comparison that
keeps an unchanged view from being sent leaves both out (row S9). Nor does a follower recover an unfinished approval-mode change,
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
socket closes; so must the subscribe reply, by the request's reply deadline
(row S31): written after
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
so a session that may not follow registers no watch,
and again before every read, once the batch holds its `requests` permit: a
request holds its capacity before it is admitted (`dispatch` does the same), so
nothing revoked while a batch waits for a permit is read (row S30). An unsubscribe is admitted by the socket before
it changes anything (row S29). Both use the same admission as `dispatch`
(`admit_now`). Both targets ask the read grant there too: a view batch for its
conversation, and a list batch, which is refused to a paired device
([read grants](read-grants.md)); nothing else decides whether a frame may be read.

### Limits

`x-subscriptionLimits` publishes `conversationTargets` (8) and `listTargets`
(two: active and archived) per connection, and `deliveryTimeoutMs` (10000).
Those per-connection limits are the only count: the SDK and catalogue watch
registries index interest by conversation and owner and set no gateway-wide
bound of their own (#978). A list frame larger than the 64 KiB frame bound is
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

S8 separates the shared loop’s coalescing from durable view replay. Its catalog
regression finishes filesystem setup before pausing time, observes the first
read while the writer is held, then changes the remaining state synchronously.
S7 retains actual committed views and monotonic cursors. A writer deliberately
held through real durable I/O can reach S10 instead; S8 makes no storage-speed
guarantee (#935).

| Row | State and input | Result | Test |
|---|---|---|---|
| S1 | Subscribe to a conversation | Wake sources registered, first read, then the reply, then the first frame | `subscribe_replies_after_the_first_read_then_sends_the_view` |
| S2 | Subscribe; the first read is refused | The reply carries the read's code; nothing is registered | `a_refused_first_read_refuses_the_subscription` |
| S3 | `after` names a position past the store's head | Refused `cursor_ahead` | `a_cursor_ahead_of_history_is_refused` |
| S4 | `after` from another incarnation | The current view is sent | `a_cursor_from_another_incarnation_gets_the_current_view` |
| S5 | A long history and an `after` inside it | Replayed in bounded reads, one frame in flight; no frame behind `after` | `a_long_history_replays_in_bounded_frames_and_never_behind_the_cursor` |
| S6 | A commit after the first frame | One new frame | `a_commit_after_subscribe_is_delivered` |
| S7 | Commits racing the subscription's reads | The last frame shows the last commit; cursors only move forward | `replay_then_live_misses_no_commit_under_concurrent_writes` |
| S8 | Many changes while a frame waits for the writer, before the delivery deadline | One next frame, with the latest state | `changes_while_a_catalog_frame_waits_collapse_into_one_frame` (shared subscription loop, controlled in-memory changes); `replay_then_live_misses_no_commit_under_concurrent_writes` (real-store view/cursor integration) |
| S9 | A wake that changes nothing the client sees (the revision and `serverNowMs` alone may differ) | No frame | `a_wake_that_changes_nothing_sends_nothing`; `views_that_differ_only_in_revision_have_one_key` (unit) |
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
| S20 | A pending approval request changes the overlay; later native acknowledgement commits the configuration | Pending frame retains the committed preset and cursor. Confirmed configuration publishes the new preset and advances committed progress | `an_overlay_change_without_a_commit_is_delivered`, `a_confirmed_mode_change_is_delivered_with_its_committed_cursor` |
| S21 | The socket closes | Every task ends and every registration is dropped | `closing_the_socket_drops_every_subscription` |
| S22 | A frame larger than the frame bound | Ended `too_large` | `an_oversized_view_ends_the_subscription_as_too_large` (unit, `product/subscription/target.rs`) |
| S23 | A read refused for want of a storage read slot | Tried again with a doubling wait, then refused | `a_read_without_a_storage_slot_is_tried_again_then_refused` (unit, `product/subscription/target.rs`) |
| S24 | The writer took a frame and is still writing it when its delivery deadline passes | Written; not ended `lagging` | `a_frame_taken_as_its_deadline_passes_is_written_and_ends_nothing` (unit, `product/subscription/target.rs`) |
| S25 | The first read has not finished by the subscribe request's passive read deadline (`readTimeoutMs`), so the rest of its delivery budget is left for the reply (S31) | Refused `unavailable`; its wake sources go with it | `a_first_read_past_the_reply_deadline_is_refused_unavailable` (unit, `product/subscription/target.rs`) |
| S26 | A followed conversation is closed or stopped (its agent let go) | One frame: the same history, read-only, lifecycle `absent`; the agent is not opened again until a send opens it | `a_closed_conversation_is_shown_let_go_and_not_opened_again` |
| S27 | A followed conversation has an unfinished approval-mode change | The view shows the change pending; recovery is not run and the agent is not opened | `a_pending_mode_change_is_shown_not_recovered_by_a_follower` |
| S28 | The agent's opening fails while a follower waits on it (or failed and holds its slot) | The follower is shown the committed history, read-only; `conversation.read` and a send report the failure | `a_failed_opening_holding_its_slot_is_read_by_a_follower_as_its_history` (`tests/conversation/desktop_stop.rs`) |
| S29 | A subscribe or unsubscribe the session may not make (grant, presence, or no longer current) | Refused with the admission's code; a subscribe leaves no watch registered | `a_forbidden_subscribe_is_refused_before_it_takes_a_watch`, `a_forbidden_unsubscribe_is_refused` |
| S30 | Every request permit taken when a batch is woken; the grant is revoked while it waits for one | Ended `refused` with `forbidden`; no frame is read under the old grant, because a batch is admitted only once it holds its capacity, as `dispatch` admits a request | `a_grant_revoked_while_a_batch_waits_for_capacity_ends_it_before_the_read` |
| S31 | The subscribe reply is not written by the request's reply deadline (the writer held behind other traffic) | The socket closes; the client reconnects and subscribes again, never left with a live subscription it cannot name | `a_subscribe_reply_not_written_by_its_deadline_closes_the_socket`; `the_reply_and_the_last_frame_carry_a_deadline_through_their_write` (unit, `product/subscription/delivery.rs`) |
| L1 | List subscription; a catalogue change | A new list frame | `a_list_subscription_follows_the_catalogue` |
| L2 | A turn starts or ends without a summary change | A new list frame with `running` changed | `a_turn_without_a_summary_change_updates_the_list` |
| L3 | A list larger than one frame | Cut newest first, `complete: false` | `a_list_too_large_for_one_frame_is_cut_and_marked_incomplete` (unit) |
| L4 | A stream of commits under a list subscription | List frames no closer than `LIST_REREAD_FLOOR` | `list_frames_come_no_closer_than_the_reread_floor` |
| L5 | An incomplete list; a row the frame left out changes: by the catalogue (archived, deleted), or by a commit alone (its `running`, within the service's bound), so the rows it carries are the same | A frame all the same, so the client walks the catalogue (D18); a commit that changes no row the service read sends none; notices already waiting when a list reads are taken by that read | `a_change_the_incomplete_list_leaves_out_is_still_sent`; `a_commit_to_a_row_the_frame_cut_is_still_a_change`, `a_list_takes_every_waiting_notice_before_it_reads` (unit, `product/subscription/target.rs`) |
| L6 | A paired device lists or follows with only the pairing read grant; newer ungranted rows exceed the list bound | SQL narrows before sorting, limiting and completeness; Cedar admits the catalogue and each returned candidate; the result and its scoped catalogue head share one metadata snapshot | `paired_lists_filter_before_the_bound`; `paired_lists_start_empty_without_grants` |
| L7 | An incomplete paired list; an ungranted catalogue change wakes the source, then a granted omitted row is archived | The unrelated change sends nothing; the granted change sends the replacement even when the visible rows stay the same. The comparison uses the snapshot's scoped head, not the owner's wake counter | `an_incomplete_paired_list_ignores_ungranted_wakes` |
| L8 | A paired list is registered and waits for request capacity; its caller disconnects, or its credential loses read admission before capacity returns | Disconnect retires watches without taking capacity. Lost admission refuses the first batch, without sending rows, and retires its watches | `a_cancelled_paired_list_releases_its_sources_while_capacity_is_held`; `a_paired_list_rechecks_access_after_waiting_for_capacity` |
| L9 | A paired list captures its head; another metadata connection commits a summary before the list reads its rows | Head and rows remain from the first snapshot; a later list observes the new head and row together | `a_paired_list_reads_its_head_and_rows_from_one_snapshot` |
| L10 | Catalogue admission succeeds but the configured policy denies or cannot decide a returned candidate | Refuse the whole batch without rows, cursor, scoped head or completeness; no partial disclosure from an inconsistent index | `a_paired_list_uses_the_configured_read_policy` |
| L11 | A paired observation spans a page boundary while ungranted rows change | Keep the existing scoped cursor, return each granted identity once, and finish without exposing ungranted rows | `a_paired_list_observation_keeps_its_scoped_cursor` |
| L12 | A list or observation reaches a gateway with no conversation service | Ask catalogue policy before exposing service availability: a peer remains forbidden, while an admitted device receives conversations_not_configured | `a_paired_list_admits_its_scope_before_service_availability` |
| L13 | A seeded mock-agent socket run mixes hidden summary writes, omitted-row R1→R2→R3 replacement, revoke/regrant, archive/restore, running turns and reconnects; repeat at raised counts | Each batch matches an independent granted-row ordering, ungranted activity stays quiet, former-receiver heads stop at replacement, running settles, and reconnect releases watches/capacity | `paired_catalogue_seeded_mock_agent_stress`; `paired_catalogue_seeded_mock_agent_stress_under_load` (socket coverage; L14 adds the canonical stress step through a native client) |
| L13a | A completed turn's reply-summary publication is held while its idle list frame is delivered; then the summary is released before a hidden write | Transcript completion and the first idle frame do not finish the stress turn: validate each frame until the exact final reply preview is present with `running=false`. The later hidden write leaves the paired head unchanged and the subscription quiet | `an_idle_list_before_the_reply_summary_does_not_finish_the_stress_turn` |
| L14 | A real native-client product child lists and subscribes through the pinned paired listener while hidden rows exceed the list bound; repeat replacement, restore, revoke and archive cycles | List/read/observe expose exactly granted active identities; hidden updates and R2→R3 stay quiet; R1→R2, restore, revoke and archive publish the expected replacement; a fresh child catches up | `native_paired_catalogue_stress` (canonical conversation stress invokes this exact test with default and raised cycle counts) |
| L15 | After an initial paired list is delivered, its journal-head query becomes unreadable and a summary mutation commits | The owner wake triggers a fresh read; the actual WebSocket receives `subscriptionEnded` with `refused` and `conversation_storage_unavailable`, releases the source, and remains connected | `a_paired_list_ends_with_storage_failure_after_its_initial_read` |
| AC1 | Subscribe to the agent catalog; on a gateway that pairs no environments, too | The reply, then one `agents.listed` frame, the generated shape, carrying what `agents.list` answers; with no environments paired nothing changes it, and no other frame comes | `the_agent_catalog_is_sent_again_as_paired_environments_come_and_go`; `a_gateway_pairing_no_environments_sends_its_catalog_once`; `an_agent_catalog_frame_is_the_generated_shape` |
| AC2 | A paired environment connects, disconnects or is revoked; or the count moves with the catalog unchanged (a connection taking another's place) | A frame with the catalog as `agents.list` answers it then; none when it is the same | `the_agent_catalog_is_sent_again_as_paired_environments_come_and_go` |
| AC3 | Many changes while a catalog frame waits for the writer | At most one more frame, with the latest catalog | `changes_while_a_catalog_frame_waits_collapse_into_one_frame` |
| AC4 | Paired environments come and go under a list subscription and the agent catalog's | The conversation catalogue's head does not move; the list sends nothing | `paired_environments_coming_and_going_never_move_the_conversation_catalogue` |
| AC5 | A second `agents.subscribe` on one connection; the socket closes | Refused `subscription_duplicate`; everything the subscription held goes with its socket | `a_second_agent_catalog_subscription_is_a_duplicate` |
| AC6 | A session without `server.read` subscribes, a paired reader's credential among them; the grant is lost while it follows | Refused `forbidden`, as `agents.list` is; ended `refused` with `forbidden` before the next read | `following_the_agent_catalog_is_admitted_as_reading_it_is` |
| AC7 | `conversation.unsubscribe` of the catalog's subscription, `agents.unsubscribe` of a list's; `agents.unsubscribe` by a session holding only `server.read` | `unknown_subscription` for the other's; its own is stopped, and nothing of it follows the reply | `each_unsubscribe_stops_only_its_own_kind` |
| AC8 | The paired environments' change source closes | Ended `source_closed` | `the_environments_source_closing_ends_the_catalog_subscription` |
| AC9 | A native hooks command | A frame with the catalog as `agents.list` answers it then, carrying the choice in effect | `the_agent_catalog_is_sent_again_when_the_hooks_choice_changes` |

The L13 fixtures emit test progress directly to stderr, outside libtest's
per-test output capture: seed, elapsed milliseconds, static phase, and a row,
step, cycle, or page ordinal. Setup checkpoints count synthetic rows without
printing row identities, messages, cursors, or credentials. A returned-seed
checkpoint follows return from that seed future; neither it nor a test-body
checkpoint proves background-worker or Tokio-runtime shutdown. These
checkpoints distinguish an incomplete run from an assertion failure without
changing its deadlines or assertions.


Desktop rows (`src/desktop/workspace/adapters/gateway/gateway-source.test.ts`;
the test names begin with the row id). The adapter subscribes only: it sends
no `conversation.read` or `conversation.list`, and the `GatewayClient` type it
asks of the client does not offer them.

| Row | State and input | Result |
|---|---|---|
| D1 | First listener | One list subscription; nothing asked on a timer |
| D2 | A followed conversation's frames | A frame a person would see differently is the next transcript, whatever its revision (a title, a permission ask, an app's review change without it); one that differs only in its revision or `serverNowMs` is the same transcript |
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

A changed preset intent notifies live followers after its exact durable admission and
before calling the native harness. While that call is held, followers retain
the committed preset and cursor and show the pending request. The native
acknowledgement then records its SDK configuration fact and advances the cursor.
S20 exercises these separately through a held public preset call and a completed
call; raw metadata writes do not impersonate the service notification owner.
An unchanged preset keeps the same owner, committed cursor and capabilities
without a transient pending overlay or a native setting call.

## Committed event subscriptions

`conversation.subscribeEvents` follows semantic facts for SDK consumers. It is
separate from replacement views: observations are delivered in source order,
never collapsed into the latest state. The default start is the captured committed
head; `after` resumes a saved event cursor and `replay: true` starts at the beginning.
The SDK reader certifies complete saves before publishing their changes. Neither
reading nor following starts a provider. Callbacks run in the consuming application,
not in the Agent's transition or storage task.

An event cursor is opaque, binds the exact session/incarnation/change, and doubles
as a stable event identity. Consumers checkpoint only applied batches; socket
write acknowledgement does not prove application processing. Replay can duplicate
an event. Event-derived command request IDs recover original receipts; uncertain
restored work is not redispatched. `kinds` filters metadata on the gateway; arguments,
results and thinking are not included. Existing owner policy applies to every batch.

| Row | Ordering | Required result |
| --- | --- | --- |
| E1 | Save units exist without SaveComplete | No event from that save |
| E2 | Subscribe, commit during initial read, then live | Ordered replay followed by new events, no missed committed fact |
| E3 | Consumer reconnects with applied cursor | Resume strictly after its event; duplicate-safe IDs |
| E4 | Cursor invalid, foreign, reset, or pruned | Typed refusal; no silent replacement or trigger loss |
| E5 | Writer/client is slow | Bounded batch and one frame offered; lag ends only this subscription |
| E6 | Access revoked while waiting for read capacity | Refused before the next read |
| E7 | Unsubscribe/disconnect races an offer | Same retirement and physical write ordering as view subscriptions |
| E8 | Source contains a semantic gap | Ended refused with event_history_gap; consumer chooses recovery |
| E9 | Filter matches nothing in a committed batch | Advance its safe cursor without exposing hidden payload |
| E10 | Source absent or closed | Following opens no Agent; source closure ends the subscription |
| E11 | Handler issues another command | It uses ordinary admission/scheduling; handlers cannot mutate a transition |
| E12 | Provider Finished precedes local settlement | No turn.ended until a local settlement is committed |
| E13 | Paired read credential follows events | Same follow admission and target read grant as views; unshared targets stay hidden |
| E14 | Reader capacity is exhausted, then released or access revoked | Bounded shared backoff; release request capacity and freshly authorize each retry; cancellation drops the wait |
| E15 | Adapter returns a page or Idle after a start/prior checkpoint | Shared conversation application EventProgress correlates replies with After and prior checkpoints for every event consumer: same session/incarnation, exclusive page progress, nondecreasing Idle; first valid output establishes continuity for Beginning/Now; contradictions refuse before delivery |
| E16 | Client receives differing event identity and cursor | Reject the batch before invoking its handler |
| E17 | First-party owner phone uses events or authoritative peek | Canonical purpose policy explicitly permits these read operations; ordinary current target admission still applies, while administrative methods remain denied |
| E18 | Terminal frame has both cursor types, or a cursor for another subscription kind, before or after open reply | Reject contradictory cursor combinations at validation; correlate the remaining cursor with the registered kind before terminal callbacks; end invalid_frame and release the handle before exposing wrong evidence |
| E19 | Invalid event subscribe options arrive before admission | Return a rejected Promise with TypeError, without sending an RPC |
| E20 | Owner phone peeks while target policy allows Read but denies Act | Use the shared command-target Cedar admission before reading active execution; no lifecycle read after refusal |
| E21 | Paired credential holds only conversation.read; current share changes from absent to Read to Drive before peek and explicit stop | Absent targets stay hidden; Read may observe the authoritative execution but stop is forbidden; Drive may observe and stop that exact execution. Peek uses the existing paired-read grant owner, then its current target Read check; stop keeps Drive admission. Owner-phone purpose and Act checks, unpaired manifest grants, and administrative refusals remain in force |
| E22 | An event source read is in flight when credential, paired share, or conversation access is revoked or the target is deleted | Reauthorize Page and Idle through the existing current credential and target policy owners before returning them for publication; refuse without exposing events or a new cursor. Unchanged authority preserves the output. Preparing carries no publication and retains pre-read admission |
| E23 | A raw event subscribe supplies empty, duplicate, excessive, unknown or wrongly typed kinds, or an explicit null optional parameter | Refuse invalid_request before source registration/admission. Schema-owned filter bounds/uniqueness and non-null optional field names are generated for transport validation; omitted or a unique bounded subset remains valid |
| E24 | A semantic kind has missing or contradictory metadata, live or buffered before open reply | The canonical event schema owns required/forbidden metadata and status/outcome values; generated client rules refuse the batch before callbacks or retaining checkpoint evidence. Sparse tool updates may omit name/status; state changes retain their distinct lifecycle, permission, and settlement-correction shapes |

The event transport contract owns both subscription filter cardinality and the
relationships between event kind and metadata. JSON Schema conditionals describe
the supported semantic shapes; generation publishes those same declarations for
client validation, rather than maintaining a second kind table. Gateway
projection and valid fixtures cover every projected shape. Invalid frames end
the client subscription and release its actual handle without publishing a batch.

Event subscriptions admit publication after a successful Page or Idle read, while
still holding request capacity. This final admission refreshes the credential,
paired target grant, and application target access through their existing owners.
It is the semantic batch's publication admission point, not an atomic check with
the later socket write; already admitted writes may finish. View and list
subscriptions retain their existing admission behavior.

Ordinary peek requests and subscription admission use the same paired-read
credential admission helper. Ordinary requests ask it after owner-phone purpose
admission and before falling back to the manifest grant; subscription admission
asks it after refreshing the session. This helper proves only the credential's
published paired-read grant, with the existing write-grant fallback. It grants no
target access: peek still asks current target Read admission and owner-phone Act
admission, while an explicit stop still asks the paired Drive action on that
target. An unpaired request keeps its manifest grant, and administrative methods
are outside this paired-read exception. This splits credential and target facts
between their existing owners rather than introducing a second grant evaluator.

The socket owns subscription tasks; a reconnect recreates a subscription from the
consumer's cursor. The SDK owns the semantic reader and its bounded storage work.
The conversation application owns cross-read cursor continuity through EventProgress;
consumers check each successful reader output before projection or retaining an
Idle checkpoint. SDK page construction validates the page's internal evidence,
while this shared validator correlates it with the requested start and preceding
outputs. Its regressions live in `tests/conversation/event_read.rs`.
No persisted subscription registry, plugin SDK, or workflow engine is introduced.
