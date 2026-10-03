# An MCP App's calls, held to policy and audited

An MCP App ([ADR 344](../adr/todo/344-mcp-ui.md)) reaches its own server
through the gateway: `mcp.callTool` and `mcp.readResource`, on the
conversation's own session of that server
([one connection per harness session](mcp-connections.md)). Its host releases
one mount of it with `mcp.releaseApp`. It speaks in its conversation with
`mcp.sendMessage` and `mcp.updateModelContext` (#390, "An app in its
conversation" below). The wire contract is
[protocol/README.md](../../protocol/README.md#an-mcp-apps-calls); this is what
the gateway does with it (#348, part b).

## What is built

- **Policy** (`mcp_servers::domain::app_call`): pure rules over what the
  conversation's view says of the app and what its server's session listed.
- **The flow** (`conversation::application::service::app_calls`): policy,
  then a review for a destructive tool, then the call, each step audited
  before its effect is reported. Each call runs on a task of its own, holding
  one of the gateway's 32 slots until that task ends.
- **Reviews** (`conversation::application::app_reviews`): gateway-owned,
  shown in the conversation's `permissions` beside the agent's with
  `origin: {kind: "app", server, tool}`, and answered through
  `conversation.answer` and `conversation.cancel`.
- **Audit** (`conversation::infrastructure::mcp_app_audit`): one durable
  record per step, keyed by a call id the gateway mints.
- **Resource tickets** (`mcp_servers::infrastructure::resource_tickets`) and
  `GET /mcp-resources` (`mcp_servers::entrypoint::http`).
- **The socket's app lane** (`product::socket`): 4 calls per socket;
  `mcp.releaseApp` on the control lane.
- **The client** (`packages/nessa-client`, `client.mcpApps`).

## Who the app is

The app is the tool call whose UI it is (`executionId`, `toolId`), in the
conversation the authenticated caller names, and the host's own `instanceId`
for one mount of it. It is not authenticated beyond the caller's credential:
every step is recorded as the app's, on behalf of that caller. A session token
a stand-in presents is never taken as naming a conversation.

## Who ends what

Each step is recorded with who took it:

| Who | Initiator |
| --- | --- |
| the app | the app, on behalf of the caller of `mcp.callTool` or `mcp.readResource` |
| the person who answered a review | that person, with their request |
| the caller of `mcp.releaseApp` | that caller, with their request |
| the person who closed or deleted the conversation | that person, with their request |
| a deadline, an automatic stop, the recovery of an approval-mode change that could not be applied, the gateway stopping | the system |
| a refusal or failure because the mount was released, or the opening ended, before the call was admitted, sent or held | the system: the release or the end was another command, recorded as its caller's when it ended anything |

## A conversation's apps

Each conversation's apps have one state, kept by the service for as long as
the gateway runs and the conversation is not deleted — not by its live
agent, so it is there before an opening finishes and after a close. Under
its one lock it holds the open reviews, the mounts released, and the
conversation's openings: each opening of its agent is a new epoch, and its
end ends that epoch.

- Every app call takes the epoch of the opening it resolved. It is admitted,
  a review opens for it, and a ticket is issued for it only under the lock,
  and only while that epoch is the current one and not ended, and its mount
  is not released.
- Releasing a mount takes the lock, marks it released for the life of the
  conversation's state — across a close and a reopening — and lets go of its
  reviews and tickets. A release that comes before the conversation is open
  is kept all the same.
- Ending an opening takes the lock, ends that epoch, and lets go of every
  review and ticket.

An app call keeps only its conversation's apps and the epoch of the opening
it was admitted in — never the live agent, so a call that runs a minute
holds neither the conversation's history nor its reopening or deletion. It
is checked once more just before it is handed to the session: a release or
end that lands before that last check stops it — it is not sent, to that
opening's session or a later one's. One that lands after it finds the call
sent, as it finds any call already sent: the check and the send are not one
step, and between them the call may wait for room in the session's queue,
which the agent's own calls share, so that window is as long as that wait.
The session was chosen before it, so the call never reaches a later
opening's session. Its `Admitted` is
on record before the last check, so the evidence of a release can come
between them.

So nothing is admitted, opened or issued for a mount or an opening once it
has ended, whatever the interleaving, and nothing is sent once its last
check finds it ended. A deleted conversation's apps are kept as that —
deleted, ended, and holding nothing — once its agent's stop has been
tried, so a release or an opening racing the delete finds them deleted and
cannot build them afresh. A
conversation remembers its last 1024 released mounts. A mount released
longer ago than that is forgotten: a host gives each mount a fresh
`instanceId` and never asks in a released one's name, so only a host that
does can open work for it again. An opening whose close was asked for and
failed is ended for its apps all the same: they take no more work until it
is opened again.

## States

### A tool call

| State | Event | Next | Effect, and what is recorded |
| --- | --- | --- | --- |
| — | 32 calls already running on the gateway | — | `temporarily_unavailable`; nothing recorded, nothing asked |
| — | the app is no MCP tool call with a UI in this conversation | — | `Refused(mcp_app_unknown)` |
| — | another server than the app's | — | `Refused(mcp_server_mismatch)` |
| — | no open session of that server | — | `Refused(mcp_session_unavailable)` |
| — | its mount released, or the opening it resolved ended (checked after the rows above) | — | `Refused(mcp_cancelled)`, by the system |
| — | the tool is not listed, or its `visibility` excludes `app` | — | `Refused(mcp_tool_not_for_app)` |
| — | arguments past 32 KiB | — | `Refused(mcp_request_too_large)` |
| — | arguments that are not one JSON object | — | `Refused(invalid_request)` |
| — | admitted, not destructive | Sending | `Admitted`; the arguments sent as parsed |
| — | a request the schema refuses — a name, identity or URI past its bound, a malformed reference | — | `invalid_request`, before the service; nothing recorded, as nothing names a call |
| — | admitted, destructive (`readOnlyHint` not true and `destructiveHint` not false) | Waiting | `ApprovalRequested`, then the review is shown, its arguments the canonical encoding of what will be sent |
| — | admitted, destructive, its review past 16 000 bytes encoded | — | `Refused(mcp_request_too_large)`; no review |
| — | admitted, destructive, the open app reviews already hold 16 000 bytes, or 16 reviews | — | `Withdrawn(RequestCancelled)` by the system; `temporarily_unavailable` |
| — | admitted, destructive, its mount released or its opening ended meanwhile | — | `Withdrawn(AppTornDown)` or `Withdrawn(ConversationEnded)`, by the system; `mcp_cancelled` |
| Waiting | the person allows | Checking | `Approved`, by that person and their request |
| Waiting | the person denies, or cancels the review | — | `Denied`, by that person; `mcp_approval_denied` |
| Waiting | `x-mcpAppCallTiming.reviewDeadlineMs` with no answer | — | `Expired`, by the system; `mcp_approval_expired` |
| Waiting | the caller goes (its socket closes) | — | `Withdrawn(RequestCancelled)`, by the app; `mcp_cancelled` |
| Waiting | `mcp.releaseApp` for its mount | — | `Withdrawn(AppTornDown)`, by the releaser; `mcp_cancelled` |
| Waiting | the conversation is closed or deleted | — | `Withdrawn(ConversationEnded)`, by that person; `mcp_cancelled` |
| Waiting | the conversation's agent is stopped otherwise — by the desktop, or to recover an approval-mode change that could not be applied — or the gateway stops | — | `Withdrawn(ConversationEnded)`, by the system; `mcp_cancelled` |
| Waiting | an answer and the deadline at once | — | whichever ended the review first; an answer is never lost to the expiry |
| Checking | the tool is still listed, for apps, and as destructive as it was | Sending | — |
| Checking | its mount released or its opening ended since it was admitted, before its last check | — | `Refused(mcp_cancelled)`, by the system; nothing sent |
| Sending, before its last check (not destructive) | its mount released or its opening ended since it was admitted | — | `Refused(mcp_cancelled)`, by the system; nothing sent |
| Sending, past its last check | its mount released or its opening ended | Sending | nothing of it ended: it may still reach the server, and its own `Completed` is recorded |
| Checking | it is not | — | `Refused(mcp_tool_not_for_app)`; nothing sent |
| Sending | the session cannot take another request now; nothing is sent | — | `Refused(temporarily_unavailable)` |
| Sending | the server answers within 56 KiB, measured as the JSON string the wire carries | — | `Completed(Answered{isError, bytes})`; the answer, re-encoded |
| Sending | past 56 KiB so measured | — | `Completed(Failed(mcp_result_too_large))` |
| Sending | a JSON-RPC error | — | `Completed(Failed(mcp_remote_error))`; its code, if within ±(2^53−1), and message as details |
| Sending | an answer that is no MCP answer | — | `Completed(Failed(mcp_remote_error))`, no details |
| Sending | no answer within `x-mcpAppCallTiming.callTimeoutMs` | — | `Completed(Failed(mcp_timed_out))` |
| Sending | the session ends | — | `Completed(Failed(mcp_session_unavailable))` |
| Sending | the caller goes | Sending | the call finishes on its own task and is recorded; the answer goes nowhere |
| any but Sending | a record cannot be written | — | `audit_unavailable`; the step it would have recorded is not taken |
| Sending | `Completed` cannot be written | — | `audit_unavailable`: the call was made, and its answer is withheld |

### A review

Its answer: `conversation.answer` with `allow` or `deny`, or
`conversation.cancel` (a denial). An answer to a review that has ended, or
with an option it did not offer, or naming another execution, is
`stale_permission` and changes nothing. An identity that names no open app
review is the agent's to answer, so an agent's review is never mistaken for
an app's, whatever it is named. An approval-mode change that is applied
leaves the conversation's apps as they are: the mode is trust in the agent,
not in an app.

An app review is shown only in a view whose transcript is confirmed
complete: the client refuses a view of unconfirmed history that offers any
control. The agent's own reviews and questions come first, exactly as the
view shows them with no app review open: an app's server must not be able to
hide or displace them. The oldest app reviews that fit beside them are shown,
at most 16 000 bytes together, room made out of the view's transcript, tool
calls and queue. From the first that does not fit on, they wait unseen and
expire if nobody answers them. The view says so where that notice fits beside
the agent's own and the view says nothing more specific of its own; the
notice's room, too, comes out of the transcript, tool calls and queue. The
view's revision, an opaque token, is replaced by a digest of it, of every app
review open, shown or not, and of how many are shown, no longer than the
revision it replaces: the same on every read of the same state, and changed by
any change in which are open or shown.

### A resource read

| Event | Effect, and what is recorded |
| --- | --- |
| refused as a tool call is (app, server, mount released, opening ended) | the same codes |
| a URI that is no `ui://` resource, or past 2048 bytes | `Refused(invalid_request)` |
| admitted | `Admitted`; the resource read |
| admitted, its mount released or its opening ended before its last check | `Refused(mcp_cancelled)`, by the system; nothing read |
| no open session, or it ends | `Completed(Failed(mcp_session_unavailable))` |
| read, and not an app's HTML | `Completed(Failed(mcp_app_unknown))` |
| no answer within `x-mcpAppCallTiming.readTimeoutMs` | `Completed(Failed(mcp_timed_out))` |
| the session cannot take another request now; nothing is sent | `Refused(temporarily_unavailable)` |
| read, its mount released or its opening ended meanwhile | `Completed(Failed(mcp_cancelled))`, by the system; nothing held |
| read, its URI, CSP and domain past 48 KiB encoded, more than a response carries beside them | `Completed(Failed(mcp_result_too_large))`; nothing held |
| read, no room to hold it: 16 MiB or 64 tickets per conversation | `Completed(Failed(temporarily_unavailable))` |
| read and held, pending | `Completed(Answered)`, then `TicketIssued{digest, size, sha256}`, then the ticket is made redeemable and answered |
| either record cannot be written | the pending ticket is discarded, unreported; `audit_unavailable` |
| released between `TicketIssued` and being made redeemable | `TicketEnded` with that cause and initiator; `mcp_cancelled` |

### A ticket

A ticket is pending from its issue until `TicketIssued` is on record: it
cannot be redeemed, and if it is let go meanwhile its end is not reported by
the store but by the read that issued it, after its issue — how and by whom
it ended, kept until the read asks, however long its records took. So no
ticket's end is ever on record before its issue. Its lifetime (`expiresInMs`) runs from its issue,
so the host has at most that long. An issuer that panics between holding its
ticket and recording its issue leaves its pending end kept until the
gateway stops; nothing reports it.

| Event | Effect, and what is recorded |
| --- | --- |
| `GET /mcp-resources` with it, within its lifetime (`expiresInMs`) of its issue | `TicketRedeemed`, by the app, recorded before the bytes are served |
| redeemed, and that cannot be recorded | `503`, nothing served, the ticket spent |
| redeemed again, expired, released, pending, never issued | the same empty `404` |
| its lifetime passes | `TicketEnded(Expired)`, by the system |
| its mount released | `TicketEnded(AppReleased)`, by the releaser |
| its conversation closed or deleted | `TicketEnded(ConversationEnded)`, by that person |
| its conversation's agent stopped otherwise, or the gateway stops | `TicketEnded(ConversationEnded)`, by the system |

### The gateway stopping

1. Every live conversation's apps are ended first, by the system: reviews
   withdrawn, tickets released.
2. The agents are stopped.
3. The gateway waits up to 10 s for every app call's task to end, so each
   records its last step.
4. The ticket ends are recorded before the ticket recorder stops, for up to
   10 s.

Anything still running after that is logged as such.

## An app in its conversation (#390)

An app may write the person's next message (`ui/message`) and give the model
context (`ui/update-model-context`). Both are the app's steps, audited as the
app's on behalf of the caller, on the app lane.

**Decisions.**

- **Consent: the first message per mount, per opening.** It waits on an app
  review (`ReviewAsk::SendMessage`) in the conversation's `permissions`,
  answered as a destructive call's is. Allowed, the mount is remembered
  under the conversation's apps lock, and asks no more until it is released
  or the opening ends; denied, expired or withdrawn, the next message asks
  again. An opening remembers its 64 latest allowed mounts.
- **Who wrote it is part of the message.** The SDK's `UserMessage` carries
  its `MessageSender` — the person, or the app's tool call (execution and
  tool call, server and tool) — persisted with the invocation, compared on a
  retry, and projected as `ConversationMessage.app`.
- **An app's message waits for nobody.** Refused `turn_running` while a turn
  runs or input waits, checked inside the submission under the
  conversation's submission lock.
- **One context per mount, sent once.** Held under the apps lock, pending
  until its update is on record (as a ticket is), taken by the next message
  admitted into the conversation, whoever wrote it. Taken only once the agent
  has the message: a refused message takes none. A retry carries what its
  first attempt took and takes nothing more.
- **How it reaches the agent.** Claude, Codex and OpenCode are all ACP
  harnesses; each is given one leading `text` block — a fixed preamble, then
  the contexts as one JSON array (`prompt_content.rs`). A text block is the
  one kind every ACP agent takes, and JSON encoding means nothing an app
  writes can end the block or pass for another app's entry.
- **Bounds.** A message: the conversation's input bound. A context: 8 KiB of
  text and structured JSON together (`AppContext::MAX_BYTES`); at most 4
  mounts hold one (`UserMessage::MAX_APP_CONTEXTS`), so a turn carries at
  most 32 KiB of app context.

### A message

| # | State | Event | Next | Effect, and what is recorded |
| --- | --- | --- | --- | --- |
| M1 | — | the app lane or the 32 slots full | — | `temporarily_unavailable`; nothing recorded |
| M2 | — | no app of this conversation, or another server | — | `Refused(mcp_app_unknown / mcp_server_mismatch)` |
| M3 | — | blank text | — | `Refused(invalid_request)` |
| M4 | — | past the input bound | — | `Refused(mcp_request_too_large)` |
| M5 | — | its mount released, or the opening ended | — | `Refused(mcp_cancelled)`, by the system |
| M6 | — | mount not yet allowed | Waiting | `ApprovalRequested`; the review shows `{"text": …}` |
| M7 | Waiting | the person allows | Sending | `Approved`, by that person; the mount allowed |
| M8 | Waiting | denied, expired, or withdrawn (its request gone, a release, an end) | — | as a call's review; not allowed |
| M9 | — | mount allowed | Sending | `Admitted` |
| M10 | Sending | released or ended before its last check | — | `Refused(mcp_cancelled)`, by the system; not sent |
| M11 | Sending | a turn running or input waiting | — | `Refused(turn_running)` |
| M12 | Sending | the agent takes it | — | `MessageSent{executionId}`; answered with it |
| M13 | Sending | the conversation refuses it | — | `MessageNotSent{executionId}`; its own code |
| M14 | Sending | the agent took it, its admission evidence failed | — | `MessageSent`; the evidence's code |
| M15 | any | a record cannot be written | — | `audit_unavailable`; the step not taken |
| M16 | allowed | release, or the opening ends | — | allowing forgotten |

### A context

| # | State | Event | Next | Effect, and what is recorded |
| --- | --- | --- | --- | --- |
| C1 | — | lane or slots full | — | `temporarily_unavailable` |
| C2 | — | no app, another server, released, ended | — | as M2, M5 |
| C3 | — | past 8 KiB | — | `Refused(mcp_request_too_large)` |
| C4 | — | structured content no object | — | `Refused(invalid_request)` |
| C5 | none or held | an update, fewer than 4 other mounts holding one | pending | its place taken; then `ContextHeld{bytes}`; then held, replacing the mount's |
| C6 | none | 4 other mounts hold one (pending ones too) | none | `Refused(temporarily_unavailable)` |
| C7 | any | an update with neither part | none | `ContextCleared`; held and pending let go of |
| C8 | held | a message admitted | none | carried with it, once, in the order given |
| C9 | held | a message refused | held | kept |
| C10 | held | a retry of a message the agent has | held | the retry carries what it first took |
| C11 | held | its mount released | none | let go of, unsent |
| C12 | held | the opening ends | none | all let go of, unsent |
| C13 | pending | a message admitted | pending | not carried: what was on record is |
| C14 | pending | `ContextHeld` cannot be written | — | let go of; `audit_unavailable`; what was held stands |
| C15 | pending | released or ended before its record | none | its record stands; the release came after it |
| C16 | pending ×2 | two updates of one mount recorded out of order | held | the later given stands |

## Lanes

App calls have 4 slots on each socket. A destructive call holds its slot while
it waits, so held calls can fill a socket's lane, but never the lanes
`conversation.read` and `conversation.answer` use, and their responses have
room of their own in the socket's response queue. `mcp.releaseApp` is a
control. A socket that goes aborts its app calls; each call's own task then
withdraws its review on record, or finishes a call already sent. The 32
gateway-wide slots are held by those tasks, not by the socket, so a caller
that reconnects cannot start calls past them. They are not shared out by
person: a gateway serves its one owner, whose apps they all are.

## Tests

Each row above has a test, named after it:

- Policy, one per row: `crates/nessa-server/tests/mcp_servers/app_call.rs`.
- Reviews: `crates/nessa-server/tests/conversation/app_reviews.rs`.
- The flow, each row of "A tool call" and "A resource read":
  `crates/nessa-server/tests/conversation/app_calls.rs`.
- Audit records: `crates/nessa-server/tests/conversation/mcp_app_audit.rs`.
- Tickets and the route: `crates/nessa-server/tests/mcp_servers/resource_tickets.rs`,
  `http.rs`, and `gateway.rs` (a redeemed ticket in no log line or trace).
- Lanes over the socket: `mcp_app_lane` in
  `crates/nessa-server/tests/mcp_servers/gateway.rs`.
- Bounds and codes the schema states again:
  `crates/nessa-server/tests/conversation/agreement.rs` and `wire_errors.rs`.
- The client: `packages/nessa-client/src/presentation/mcp-apps-api.test.ts`.
- An app in its conversation, each row of "A message" and "A context":
  `crates/nessa-server/tests/conversation/app_messages.rs`, and the
  orderings C13–C16 in `app_reviews.rs`; the ACP block in
  `crates/nessa-sdk/tests/infrastructure/acp/executions/prompt_content.rs`;
  its persistence in `snapshot/semantic.rs`; the socket in
  `an_apps_messages_and_contexts_travel_on_its_lane_and_land_as_its_own`;
  the desktop in `app-messages.test.ts` and `mcp-apps.mjs --only
  message`.
