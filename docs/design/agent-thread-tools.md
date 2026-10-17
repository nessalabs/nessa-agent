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
| Saved by an earlier run, turn unfinished | Reported at start: `interrupted` when no result was saved, else that saved result and reply |
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

No turn the earlier run left unfinished runs again after a restart, so a saved
report whose turn saved no result is reported `interrupted`;
`restored_unfinished` owns that decision, and is what changes once interrupted
turns resume. A thread cannot report to itself. Two agents that keep messaging
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
