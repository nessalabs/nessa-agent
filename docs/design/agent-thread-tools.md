# Agent thread tools

The gateway offers `nessa-agents` beside configured MCP servers and the optional
`nessa-environments` server. It uses the existing stand-in relay and host-issued
provider-open token. Agent tools do not require SSH command configuration.

A thread is a conversation. `create_thread` creates an empty thread using the
configured default agent/model and normal Ask approval mode. `send_message`
accepts plain text with `mode`: `queued` (the default) queues a turn;
`steering` uses existing steering delivery and fallback semantics; `passive`
records a RecordOnly note without starting or steering a model turn. Active
modes return an execution identity and durable command receipt; passive returns
a message identity, acceptance time and note delivery state. `subscribe` returns committed event metadata and an opaque cursor;
repeat the call with `after` to continue. With no cursor it starts from now; `replay: true` reads retained events from the
beginning. `replay` and `after` are mutually exclusive. After sending a task,
use replay when there was no prior checkpoint so a fast completion is included.
`stop_thread` stops the named execution, or captures the current active execution
when `executionId` is omitted. The application resolves the running execution from its authoritative live
snapshot, independently of the bounded message view: later Injected steering
children do not hide their running parent. Queued work requires an explicit
execution identity. A stop does not delete or close the conversation.

An optional `requestId` makes an intentional command retry recoverable. Creation,
submission and stop use the existing durable command coordinators. Default
conversation/execution IDs derive from source identity, request and purpose, so
retrying creation or submission retains their target. A retry preserves thread,
text and mode. Queued and steering submissions share the existing command
namespace, so switching between them under one request conflicts. Passive notes
retain their existing separate note namespace: switching between passive and an
active mode is a different existing operation, with no cross-namespace conflict
or deduplication guarantee. Repeating a passive request with identical text returns
the original note; changed text conflicts in the note owner. A caller retrying a stop should preserve
the returned `executionId`; without it a changed active turn produces a conflict
rather than proving that the earlier stop completed. An omitted `requestId`
means a new command on each call.

## Reports back

A queued or steering `send_message` reports back when its turn ends
(`reportsBack: true`). The report is a new queued owner turn in the sending
conversation, admitted under the same caller its send was, that names the
thread and execution, the outcome, and the thread's final visible reply: the
text after its last tool step, thinking left out, cut so the whole report fits
the conversation's input limit. It queues behind whatever the sender is doing,
so a sender that ended its turn is woken, and one still running reads it next.
A passive note starts no turn and reports nothing.

`service/thread_reports.rs` owns this, and `ThreadReportStore` keeps every
waiting report durably: `LocalThreadReports`, one row per report in
`conversations/thread-reports.sqlite3` (version 1; a version this build cannot
read refuses the gateway, ADR 202). There is no count limit and no lifetime: a
report waits until its turn finishes, its thread or parent is deleted, or its
person may no longer view the thread.

The send saves and registers its report before it submits, so a turn that ends
at once is not missed, and again after a submit that succeeded, so a
concurrent retry that failed cannot let it go. A report that cannot be saved is
not promised (`reportsBack: false`). The send then looks once, so a retry of a
turn that already ended still reports. A refused send asks the thread's saved
history: with no turn under its execution, the report goes; with one, it waits
for that turn like any other.

A look happens after each invocation of a watched thread (`ThreadReportHook`),
after a watched queued turn is withdrawn, after a watched thread closes, and at
start for every thread a saved report waits on. One look runs per thread at a
time; one asked for meanwhile runs after it. Each look checks that every person
with a report waiting may still view the thread: a refusal, or a thread gone,
lets that person's reports go and tells them nothing. It reads the thread's
saved invocations, from its agent when it is open and from its committed
history when it is not. A watched execution is finished once its settlement is
saved: settled, or cancelled, or injected into the turn its scheduling names as
`target` once that turn has settled, whose outcome and reply are then its own.
The hook runs before the settlement it follows is saved, and a failed turn
makes the agent cancel the turns queued behind it after that, so a look keeps
looking for up to five seconds while a watched turn is settling.

The report's execution identity derives from the send's source, request and
purpose. The parent's own history is what says it has the report: each
delivery first reads it, and one already there is settled, not queued again; a
second submit under the same identity answers from the first. Delivery queues
the report into the parent directly, not through the command store, so it never
meets the person's own commands. Each delivery runs on its own; one the parent
cannot take yet (too many conversations open, storage not answering) tries again
with growing waits, up to a minute, until the gateway retires, and then stays
saved for the next start. A report leaves the store only once its parent has
it, or refuses it for good (deleted, gone, or an invalid message).

At start the saved reports are loaded and their threads looked at, sixteen at a
time. A report this run saved before the load read it is left to its own send,
not taken for an earlier run's.

| When | What happens |
| --- | --- |
| The send's submit succeeds | Report saved and held; one look |
| The send is refused, no turn saved | Report let go |
| The send is refused, a turn saved | Report waits for that turn |
| The report cannot be saved | `reportsBack: false`; the send goes on |
| Turn settles or is cancelled | Report queued into the parent, then let go |
| Steer injected into a turn | Reported with that turn once it settles |
| Queued turn withdrawn, or thread closed | Look; reported `cancelled` |
| Parent cannot take it yet | Retried until it is taken |
| Parent already has it (crash before settle) | Settled, not queued again |
| Gateway retires before delivery | Stays saved; the next start delivers it |
| Saved by an earlier run, turn finished | Delivered at start |
| Saved by an earlier run, turn unfinished | When automatic recovery is bound, wait for the SDK successor chain; otherwise report the interrupted run |
| Parent's agent cannot open in this run (configuration, placement) | Not retried in this run; stays saved for the next start |
| Person may no longer view the thread, or it is deleted | Let go, nothing told |

The following delivery interleavings keep the original binding and current authority:

| Ordering | Required evidence | Regression |
| --- | --- | --- |
| A send waits on child A; the same request is retried against B; gateway reopens | Conflict preserves A's durable binding; resume queues one report | `a_conflicting_retarget_preserves_the_original_durable_report` |
| Earlier-run row exists; a retarget retry arrives before startup loads it | Saved binding remains authoritative; startup owns delivery | `saved_thread_reports_are_delivered_after_a_restart` |
| Parent accepts a queued report; settling its durable row fails; gateway reopens | Parent history deduplicates delivery and the row is settled | `an_accepted_report_is_not_queued_again_after_settle_failure_and_restart` |
| Saving the report fails before a successful send | Send executes with `reportsBack: false`, without a waiting report | `a_report_save_failure_does_not_promise_a_report_or_refuse_the_send` |
| Close owns its work; its response caller is dropped before close completes | Its supervised owner still asks for the report look | `a_close_keeps_its_report_wake_after_the_caller_is_lost` |
| Child is deleted before its completion is looked at | No child payload reaches the parent; durable waiting row is removed | `a_deleted_child_is_not_reported_to_its_parent` |
| Final scheduling settlement takes longer than the first settling look window | A later look delivers without another invocation or manual wake | `a_report_follows_settlement_after_the_initial_look_window` |
| A report is taken; parent is temporarily unavailable; child is deleted before retry | Retry rechecks child authority and releases the durable report without disclosure | `a_taken_report_rechecks_its_deleted_child_before_retrying_delivery` |

Automatic recovery continues unfinished work through SDK-owned successors.
Reports follow those successors and keep the original watched request label.
With no recovery owner bound, `restored_unfinished` describes the interrupted
run instead. A thread cannot report to itself. Two agents that keep messaging
each other keep reporting to each other: that is their own conversation, and the
person stops it as they would any turn. Tests:
`a_thread_turn_reports_its_outcome_and_reply_back_to_the_sender`,
`saved_thread_reports_are_delivered_after_a_restart`,
`a_start_reading_a_report_this_run_saved_leaves_it_to_its_send`,
`a_long_reply_is_cut_so_the_whole_report_fits_the_input_limit`,
`waiting_reports_survive_reopening_until_settled`.

For example, call `send_message({threadId, message: "Review the change"})`, then
`subscribe({threadId, replay: true})`. Continue with the returned `cursor` as
`after` until `more` is false; use `waitMs` when waiting for later changes.

## Source authority

The relay supplies source session identity; tool arguments supply no owner,
source identity, credential or approval override. The conversation application
captures the source's live lease and running invocation, including its original
actor and initiation. The `conversation.agent_tools` Cedar action decides
whether it may represent the source owner. `owner.agent_tools` admits owner
initiations on their own live conversation and denies grant-origin turns. A Drive
grant allowing a tool does not acquire owner-wide authority over other threads.
Existing target Cedar admission still decides access to each target.

Source execution, lease, actor, and initiation are copied once as owned provenance
from the committed shared snapshot handle. The handle is released before the
final metadata read or audit await, so source admission retains only that
provenance during those waits. After any wait for the live map or snapshot, the service reloads source
metadata immediately before synchronous Cedar evaluation. That metadata read
supplies current owner/organization/deletion facts; the earlier existence read
is diagnostic only. A deletion fence committed during those waits is therefore
presented to Cedar rather than replaced by the earlier un-deleted record. The
snapshot's execution and actor are not selected again after the metadata read:
a later turn must not inherit this call's attribution. No conversation-wide or
deletion-cleanup lock is retained for source admission. The policy decision is
its admission point; admitted audit supervision and post-audit cancellation keep
their existing semantics.


The source decision is acknowledged in `audit/agent-tools` before target work.
It records source conversation/execution/lease, original principal/surface,
delivery call ID, raw logical request ID, source/tool scoped command ID, tool,
target and deciding policy IDs. The scoped command ID remains the action identity
passed to the existing command or note owner; the logical ID is attribution, not
a replacement idempotency namespace. Target
commands retain their ordinary receipts, audit and permission behavior. Audit
failure is visible and prevents successful admission. Its writer uses the same
immutable private-file publication and directory sync as other conversation
control audit adapters. Audit delivery does not depend on an MCP response writer.

The token identifies a provider open, which can span several turns. The running
source execution is resolved when the call is admitted; the token alone does not
prove a request was emitted by a particular earlier execution. It is an opaque
source grant, not an owner's product-session credential. Already admitted work
can finish after source cancellation, as ordinary command admission does.

## Observation and cancellation

`subscribe` holds one independent committed SDK reader, not an Agent handle. It
projects execution/tool IDs, kinds, statuses and outcomes; it omits message
content, thinking and tool arguments. A history gap or invalid/reset/pruned
cursor is an explicit refusal. A callback does not run inside a transition,
and receiving events does not cause a model turn or enqueue another message.

The tool returns up to 16 semantic facts per read step, with a maximum 256 MiB
logical input budget because a stored semantic fact may span physical records.
Only the metadata projection is returned, and tool result JSON is bounded to
32 KiB before the MCP text/structured duplication. At most four observation calls
run across the built-in tool server, leaving capacity under the relay's eight
concurrent calls per connection. Independently, one conversation service admits
at most eight physical source-admission owners across all tools. The permit is
acquired without waiting before spawning and is retained inside the detached
owner until metadata/admission/audit work physically ends. Cancelling an observer
releases its observation slot but does not free that physical permit. Further
admission refuses unavailable while all eight are retained. Shutdown still waits
for retained service admission, or reports its existing bounded drain failure.
Initial source certification has a ten-second preparation bound beginning before
source admission; setup, reauthorization and reads share cancellation and their
current phase deadline. Idle reads poll at up to 250 ms for at most `waitMs` (maximum
30 seconds), and each read rechecks current target access. All successful replies,
including an empty cached cursor at the wait deadline, pass one final current
target access check after observation. That delivery check shares source
cancellation and has its own ten-second bound: an expired observation deadline
cannot certify access. Refusal, cancellation, or an unavailable delivery check
returns an error without events or cursor; a successful delivery check can add up
to ten seconds beyond the observation wait. SDK cursor progression
makes coalesced notices harmless; the tool keeps no subscription registry.

Call cancellation, disconnect and source token revocation signal the pending
observation to stop and drop its reader. This does not stop the observed thread.
A create/send/stop already admitted by the command coordinator retains its
supervised receipt/audit lifecycle even when its answer is lost. Cancellation
is not a claim that an external effect was rolled back.

## Ordering and regression evidence

| Order or failure | Behavior and test home |
| --- | --- |
| Source running owner asks for target | Original actor and source/target attribution preserved; `tests/conversation/agent_tools.rs`. |
| Source tombstone commits while initial metadata reply/live snapshot acquisition is delayed | Fresh metadata reaches Cedar after source snapshot capture; deny before target dispatch and audit the original source execution/lease/actor/call/logical/scoped-command/target; `tests/conversation/agent_tools.rs`. |
| Source admission captures provenance, then waits for final source metadata or audit | Capture owned lease/execution/actor/initiation once from the committed shared handle and release the handle before either await. Preserve that provenance while current metadata reaches Cedar; `source_admission_reloads_metadata_after_waiting_for_live_evidence` checks no extra snapshot owner remains during the metadata wait. The SDK `shared_snapshot_handles_reuse_immutable_committed_history_without_message_clones` test owns the zero-copy handle contract; the consumer lifetime check does not count deep copies. |
| Same delayed source stays unchanged, cancels, or latest metadata read fails | Unchanged source remains admitted; cancellation retains its existing allowed decision audit but starts no target; metadata failure returns its typed error without inventing an authority decision; same tests. |
| Missing running source or call already cancelled | No source admission/audit or target effects; same tests. |
| Grant origin or foreign/deleted source | Cedar denies; complete source truth table in `tests/conversation/access_policy.rs`. |
| Admission audit fails | Typed failure before target dispatch; `tests/conversation/agent_tools.rs`. |
| Explicit logical request through Relay | Raw request and scoped command identities both reach audit; create/send/stop remain separate and another source derives a different command; `tests/conversation/agent_tools.rs`. |
| Repeated request | Existing command receipt owns active retry/conflict rules; stable defaults tested in `composition/conversation_tools.rs`. |
| Passive send and retry | RecordOnly note is saved once, changed text conflicts, and provider execution count stays unchanged; actual Relay test in `tests/conversation/agent_tools.rs`. |
| Steering and queued mode change | SDK retains Steering intent and its non-native queue fallback; queued/steering changed mode conflicts in the command fingerprint; same Relay test. |
| Default stop on a running worker | Real provider cancellation names the running execution once; explicit retry returns the original stop receipt; same Relay test. |
| Omitted optional tool argument versus explicit null, non-object arguments, object-shaped or unlisted enum values | Omission alone selects a default; every other shape outside the published schema is refused at decode. One shared presence-aware Serde decoder rejects null for every published non-null optional scalar before source admission. The schema-driven argument matrix covers all four tools; the actual Relay null-stop case leaves the active execution, cancellation count and source audit unchanged. |
| Built-in tools attach to the service after MCP composition; the gateway later shuts down and drops its owners | Stand-ins remain available while the relay is alive; the live relay retains the late-attached service even after explicit service/resolver/tool handles are dropped. After service shutdown and dropping the MCP composition too, the resolver and both built-in tool objects are released; `composed_builtin_tools_release_the_service_and_resolver_after_shutdown` in `tests/composition/current_agent.rs` exercises the actual composition graph. |
| Tools before late attachment | Explicit unavailable; same adapter tests. |
| Multiple built-ins/configured collision | Each server's own name/digest used; `tests/composition/mcp_servers.rs`. |
| Cancellation/disconnect/revoked grant | Existing relay stops affected calls; `tests/mcp_servers/built_in.rs`. |
| Incomplete save, duplicate reads, cursor recovery | SDK committed-event reader owns visibility and replay; SDK record storage events tests. |
| Slow or oversized observation | Reader permits, step/response budgets and timeout refuse explicitly. |
| Delayed stop versus newer execution | Existing exact-execution stop coordinator owns cancellation; `tests/conversation/mutation_commands.rs`. |
| Running parent followed by enough Injected steering children to leave the message window; idle with queued work | Authoritative application snapshot resolves the parent or no active turn; shared service regression and the Relay active-stop regression in `tests/conversation/agent_tools.rs`. |
| Repeated observer cancellation during blocked source metadata | Four observation slots return, but eight detached source owners retain separate permits and service admission; the ninth fails before metadata I/O, and shutdown drains only after release; same tests. |
| Target deleted or policy revoked while Page/Idle read is blocked | The final delivery check refuses before any event or cursor is returned; `tests/composition/conversation_tools_subscription.rs`. |
| Prior Idle checkpoint, then target revoked during another read, reauthorization, or idle sleep as deadline expires | The cached checkpoint still passes final delivery admission; refusal returns no prior cursor; same tests. |
| Final delivery authorization is blocked while source cancels or its delivery budget expires | Cancellation returns cancelled; ten-second delivery timeout returns unavailable, without events/cursor; same tests. |
| Read returns a typed storage/capacity refusal after target access changes | Preserve that refusal; no delivery certification or successful output is introduced; same tests. |
| Access still holds after delayed Page/Idle or observation deadline | Deliver the projected metadata or empty checkpoint after fresh authorization; same tests. |
| After checkpoint followed by another incarnation, backwards Page/Idle, duplicate Page, or Page at the retained outer boundary | Shared application EventProgress rejects event_cursor_invalid before projection or cached-cursor replacement; actual follow regressions in `tests/composition/conversation_tools_subscription.rs`. |
| Now/Beginning first Idle then Preparing and another Page/Idle | First Idle establishes continuity; Preparing retains it. Wrong incarnation and backwards progress refuse, while trailing events and nondecreasing Idle remain valid; same tests. |
| Now returns its first Page without a prior checkpoint | Accept the source's captured starting position; no prior cursor is fabricated; same tests. |
| Subscribe supplies after together with replay:true or replay:false | Reject invalid_request by field presence before source admission or reader opening; actual Relay regression in `tests/conversation/agent_tools.rs`. |

The observation loop previously treated permission to start a read as permission
to publish its eventual result. Separate return paths for page, idle, and deadline
then allowed a cached checkpoint to bypass later authority. Delivery is now one
boundary around the observation result, with the application service remaining
the single owner of target access. The source actor is the already admitted
caller: its supervised source decision/audit is not repeated during observation.
Source cancellation and provider-token revocation still flow through the relay's
watch channel; this final gate does not create a new delegation or source lease.

A page constructor validates one page, and cursor/session validation checks one
identity. Neither certifies a custom adapter's output against the requested start
or a previously observed Idle. The shared application `EventProgress` owner,
extracted from the API consumer, supplies that cross-read decision to both socket
and tool adapters. The tool consumes it before projecting a Page or replacing its
retained Idle checkpoint; no second comparison implementation is introduced.

Ordinary Serde `Option<T>` parsing erased the distinction between omission and
explicit null even though the published tool schemas allow only scalar values.
The adapter now uses one `supplied` decoder for all optional scalar fields:
Serde's default applies only when a field is absent, and a present field must
deserialize as its declared `T`. This preserves the existing typed decoder and
defaults without a second argument validator or field-specific null checks.
The argument regression derives its cases from the published tool schemas.

Serde's derived decoders also accept shapes the published schemas do not: a
struct decodes from a positional array and a unit enum variant from its map
form. So `parse` refuses any arguments that are not an object, and each published
string enum (`mode`, `agent`) decodes only from one of its listed strings. Every
schema constraint is therefore checked when the input is decoded, before source
admission or audit; nothing is validated after admission.

Implementation ownership: `composition/conversation_tools.rs` owns MCP parsing,
serialization and bounded polling; `conversation/application/service/agent_tools.rs`
owns source admission; the existing Cedar access adapter owns delegation policy;
`conversation/infrastructure/agent_tool_audit.rs` owns durable delivery. Event
projection and target admission are shared through the conversation application;
private record formats stay in SDK infrastructure.

### Reports while automatic recovery is enabled

When unfinished-turn recovery is bound, a report follows the SDK's successor
chain and keeps the original watched request label. The SDK publishes one
committed history frame with its owed identities; the server does not independently
classify which provider failures may retry. An observed admission that has not
been acknowledged cannot make a committed failed turn look final. A saved report
with no committed or observed original invocation after restart still reports
interruption: registration preceded admission and there is no admitted turn to
resume. A missing committed invocation that is observed locally waits.

| Ordering | Result | Regression |
| --- | --- | --- |
| Startup captures a mark, then a turn finishes and clears it before that snapshot returns | Each fresh submission acknowledges the existing idempotent durable mark before admission. A stale in-memory entry cannot stand in for durable evidence | `a_startup_snapshot_cannot_make_a_fresh_send_skip_its_durable_mark` |
| Startup captures an undated unfinished mark; the running turn then fails and saves a future retry before that startup snapshot is installed | Retain the newer in-process failure count and due time, including when admission renews its durable marker; the startup sweep admits no successor before that deadline, and saved retry metadata is unchanged | `a_startup_snapshot_preserves_a_newer_failure_count_and_retry_due_time` |
| A committed result survives without its final scheduling edge | Consume the SDK's finality query after resolving injection and successor ownership; report provenance does not establish whether an execution is live | `a_restored_completed_result_without_its_last_edge_is_reportable` |
| Public unfinished-turn sweep loses its caller while an already-open Drive successor waits before its input save | The owned resume keeps the actual grant revision hold through admission, then installs the receipt watcher and clears the durable mark after settlement | `an_already_open_resumption_keeps_its_grant_hold_and_watcher_after_caller_loss` |
| That caller is lost, then the successor input save fails | The owned host operation persists a failure count and due retry time, rather than leaving an undated mark stranded until restart | `an_already_open_resumption_schedules_retry_after_caller_loss_and_save_failure` |
| A failure deferral has saved its future due time, but its caller has not published that fact in the cache; startup reads that saved row | Startup does not schedule or admit a successor before the saved due time, and the acknowledged retry remains saved | `a_startup_scan_respects_a_retry_saved_before_its_cache_update` |
| Report recovery runs before unfinished-turn recovery | Keep the original report pending while the successor runs; report its final committed result once | `reports_loaded_before_unfinished_turns_wait_for_the_resumed_child` |
| Unfinished-turn recovery runs before report recovery | Same original report identity and eventual final result | `unfinished_turns_loaded_before_reports_report_the_resumed_child_once` |
| Successor is observed but its save is not acknowledged | The original cannot be reported final from an older committed frame | `an_unacknowledged_successor_does_not_end_the_committed_roots_work` |
| SDK close and the lease adapter both fail to save retained evidence through the same session writer | Return the first typed storage failure with its actual physical cleanup result; log a later write failure without nesting evidence failure in the physical-result field. Retained facts await a later acknowledged save. Confirmed cleanup releases the slot; uncertain cleanup keeps it | `l5_a_desktop_stop_whose_cleanup_record_cannot_be_saved_says_so_and_the_conversation_opens_again`; `l7_l8_a_failed_cleanup_whose_records_cannot_be_saved_answers_both_and_keeps_its_cause`; `a_lease_save_keeps_the_first_storage_failure_and_confirmed_cleanup` (later `Busy` differs from first `Io`) |


### Retry catalogue ownership

The startup scan discovers candidates. Its saved failure count and deadline may
already be old when it returns, so they do not initialize or override the live
schedule. One short per-conversation marker-update lock owns each durable
mark, exact read, deferral or clear through its cache publication. Startup reads a
current mark only when that schedule has no entry. A missing row is skipped.
Ordinary admission acknowledges its mark and receives the exact saved metadata
from the same transaction; it initializes a missing live entry from that reply.
An existing entry owns this process's later decisions, including a deadline the
retry worker already consumed. Both admission and startup wake the retry loop
when they install scheduling information. Final close attempts the durable clear
even before startup has initialized the cache: cache absence is not durable
absence. The marker lock is acquired inside these helpers, so cleanup uses the
same ordering whether its caller retains a submission guard or has released it.

When both are needed, submission ordering precedes marker ordering. No marker
operation acquires the submission lock or waits for provider work, opening or
receipt watching. A stopped slot remains fenced until final clear finishes. This
avoids threading borrowed or detached submission guards through physical close.
The retry worker consumes published deadlines under the existing cache mutex;
an already-selected attempt may still look, and the SDK decides what remains
owed. This is not cancellation of already-selected host attempts. A failed
clear keeps its cached state and logs the existing failure; SDK finality still
ends the work, as `a_persons_close_after_failure_stays_final_when_clearing_its_mark_fails`
checks. A crash after saved SDK finality but before marker clear can leave an
extra recovery look; marker metadata does not authorize reviving closed work.

This replaces snapshot merging; it adds no Unknown state, generation, retry
expiry, or second scheduler. The existing SDK scheduler remains the owner that
reselects owed work and admits a successor. Startup and the due worker may both
look at one conversation, but that does not authorize two successors. An exact
mark read costs one indexed lookup for a previously unknown candidate; ordinary
mark acknowledgement adds an indexed read in its existing write transaction.
Discovery scans the marked rows, with the existing bound on concurrent
openings. The retry loop still scans its catalogue when woken; many new future
deadlines can cause repeated scans. The hash table can retain its peak allocation
after entries clear. The added lock registry keeps live locks while an update
has holders or waiters and removes dead weak entries on the next acquisition.
Its map can retain its peak allocation, and acquisition scans its entries. It
introduces no per-history generation map.
Marks without a deadline and already-known startup entries
do not add wakeups.

| Ordering | Result | Regression |
| --- | --- | --- |
| A rejected conflicting command acknowledges a durable future retry before startup returns | Keep the actual saved count and due time, wake the retry loop, and run the successor at its deadline | `a_conflicting_retry_before_startup_preserves_the_saved_future_retry` |
| Startup captured a future retry, then explicit close clears the mark | A current exact read sees absence; no old deadline or failure count returns to the live catalogue | `a_cleared_future_retry_is_not_reinstalled_by_a_startup_snapshot` |
| A conflicting command acknowledges restored undated owed work before startup returns | The current undated entry remains eligible; startup carries the work on | `an_undated_conflicting_retry_before_startup_still_resumes_the_saved_work` |
| The retry worker consumed a due deadline before ordinary mark acknowledgement | Keep that consumed schedule; acknowledging the saved row does not reinstall its old deadline | `an_acknowledged_mark_preserves_a_retry_deadline_already_claimed_by_the_worker` |
| The retry loop is already waiting with an empty catalogue when a conflicting command acknowledges a saved due retry | That acknowledgement wakes the registered owner without relying on startup | `a_conflicting_mark_wakes_the_idle_retry_owner_without_a_startup_sweep` |
| Startup installs a future deadline while the retry loop already waits with an empty catalogue | Wake that owner; it dispatches at the saved deadline without another mark or scan | `a_startup_future_mark_wakes_an_already_idle_retry_owner` |
| A recovery opening fails, saves deferral, and pauses before publishing its cache deadline | The same marker-update owner keeps a second startup scan behind publication; no early successor is admitted | `a_failed_recovery_opening_keeps_startup_behind_its_defer_publication` |
| A final close runs before startup initializes its cache; later independent work fails | Clear the actual saved row, so the later failure starts its own count and deadline; SDK finality still ends the old work | `a_final_close_before_startup_clears_the_saved_future_retry` |
| Desktop Stop releases submission ordering for physical close; startup has captured an exact row but not returned it | Order the complete read/publication before final clear, or observe absence after clear; both durable and cached retry metadata are absent when the owners join | `a_desktop_stop_cannot_leave_a_late_exact_read_cached_after_final_clear` |
| A changed retry of a known execution acknowledges its saved mark while desktop Stop closes | Keep the SDK SubmissionConflict; final clear follows that acknowledgement and cache publication before releasing the stopped slot | `a_desktop_stop_clears_a_conflicting_commands_mark_before_releasing_its_slot` |
| A saved mark has damaged retry metadata | Exact read and mark acknowledgement refuse it; no default deadline, failure count, or admission is invented | `exact_unfinished_mark_reads_and_acknowledgements_refuse_corrupt_retry_cells` |
| A mark has gone or belongs to another conversation | Exact read creates no row and keeps IDs separate, including after reopen | `an_exact_unfinished_mark_read_is_noncreating_and_keeps_conversations_separate` |

Without a bound automatic-recovery owner, the existing interrupted-turn report
still describes the terminal outcome of that run. The recovery owner, rather
than a second report policy, decides whether continuation is enabled.
