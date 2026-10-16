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
| Missing running source or call already cancelled | No source admission/audit or target effects; same tests. |
| Grant origin or foreign/deleted source | Cedar denies; complete source truth table in `tests/conversation/access_policy.rs`. |
| Admission audit fails | Typed failure before target dispatch; `tests/conversation/agent_tools.rs`. |
| Explicit logical request through Relay | Raw request and scoped command identities both reach audit; create/send/stop remain separate and another source derives a different command; `tests/conversation/agent_tools.rs`. |
| Repeated request | Existing command receipt owns active retry/conflict rules; stable defaults tested in `composition/conversation_tools.rs`. |
| Passive send and retry | RecordOnly note is saved once, changed text conflicts, and provider execution count stays unchanged; actual Relay test in `tests/conversation/agent_tools.rs`. |
| Steering and queued mode change | SDK retains Steering intent and its non-native queue fallback; queued/steering changed mode conflicts in the command fingerprint; same Relay test. |
| Default stop on a running worker | Real provider cancellation names the running execution once; explicit retry returns the original stop receipt; same Relay test. |
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

Implementation ownership: `composition/conversation_tools.rs` owns MCP parsing,
serialization and bounded polling; `conversation/application/service/agent_tools.rs`
owns source admission; the existing Cedar access adapter owns delegation policy;
`conversation/infrastructure/agent_tool_audit.rs` owns durable delivery. Event
projection and target admission are shared through the conversation application;
private record formats stay in SDK infrastructure.
