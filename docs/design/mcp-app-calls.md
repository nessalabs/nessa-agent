# An MCP App's calls, held to policy and audited

An MCP App ([ADR 344](../adr/todo/344-mcp-ui.md)) reaches its own server
through the gateway: `mcp.callTool` and `mcp.readResource`, on the
conversation's own session of that server
([one connection per harness session](mcp-connections.md)). Its host releases
one mount of it with `mcp.releaseApp`. The wire contract is
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
| a refusal because the mount was released before the call came | the system: the release was an earlier command, recorded as its caller's when it ended anything |

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

So nothing is admitted, opened or issued for a mount or an opening once it
has ended, whatever the interleaving; a call admitted already is sent. A
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
| — | its mount released, or the opening it resolved ended | — | `Refused(mcp_cancelled)`, by the system |
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
| Waiting | 5 minutes with no answer | — | `Expired`, by the system; `mcp_approval_expired` |
| Waiting | the caller goes (its socket closes) | — | `Withdrawn(RequestCancelled)`, by the app; `mcp_cancelled` |
| Waiting | `mcp.releaseApp` for its mount | — | `Withdrawn(AppTornDown)`, by the releaser; `mcp_cancelled` |
| Waiting | the conversation is closed or deleted | — | `Withdrawn(ConversationEnded)`, by that person; `mcp_cancelled` |
| Waiting | the conversation's agent is stopped otherwise — by the desktop, or to recover an approval-mode change that could not be applied — or the gateway stops | — | `Withdrawn(ConversationEnded)`, by the system; `mcp_cancelled` |
| Waiting | an answer and the deadline at once | — | whichever ended the review first; an answer is never lost to the expiry |
| Checking | the tool is still listed, for apps, and as destructive as it was | Sending | — |
| Checking | it is not | — | `Refused(mcp_tool_not_for_app)`; nothing sent |
| Sending | the session cannot take another request now; nothing is sent | — | `Refused(temporarily_unavailable)` |
| Sending | the server answers within 56 KiB, measured as the JSON string the wire carries | — | `Completed(Answered{isError, bytes})`; the answer, re-encoded |
| Sending | past 56 KiB so measured | — | `Completed(Failed(mcp_result_too_large))` |
| Sending | a JSON-RPC error | — | `Completed(Failed(mcp_remote_error))`; its code, if within ±(2^53−1), and message as details |
| Sending | an answer that is no MCP answer | — | `Completed(Failed(mcp_remote_error))`, no details |
| Sending | no answer in 60 s | — | `Completed(Failed(mcp_timed_out))` |
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
control. A view bounded to its size gives up its transcript, its tool calls
and its queue before any review, so the app reviews — at most 16 000 bytes
together — are the last of it to go.

### A resource read

| Event | Effect, and what is recorded |
| --- | --- |
| refused as a tool call is (app, server, mount released, opening ended) | the same codes |
| a URI that is no `ui://` resource, or past 2048 bytes | `Refused(invalid_request)` |
| admitted | `Admitted`; the resource read |
| no open session, or it ends | `Completed(Failed(mcp_session_unavailable))` |
| read, and not an app's HTML | `Completed(Failed(mcp_app_unknown))` |
| the session cannot take another request now; nothing is sent | `Refused(temporarily_unavailable)` |
| read, its mount released or its opening ended meanwhile | `Completed(Failed(mcp_cancelled))`; nothing held |
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
ticket's end is ever on record before its issue. Its 60 s run from its issue,
so the host has at most 60 s.

| Event | Effect, and what is recorded |
| --- | --- |
| `GET /mcp-resources` with it, within 60 s of its issue | `TicketRedeemed`, by the app, recorded before the bytes are served |
| redeemed, and that cannot be recorded | `503`, nothing served, the ticket spent |
| redeemed again, expired, released, pending, never issued | the same empty `404` |
| 60 s pass | `TicketEnded(Expired)`, by the system |
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
