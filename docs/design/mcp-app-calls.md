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

## States

### A tool call

| State | Event | Next | Effect, and what is recorded |
| --- | --- | --- | --- |
| — | 32 calls already running on the gateway | — | `temporarily_unavailable`; nothing recorded, nothing asked |
| — | the app is no MCP tool call with a UI in this conversation | — | `Refused(mcp_app_unknown)` |
| — | another server than the app's | — | `Refused(mcp_server_mismatch)` |
| — | no open session of that server | — | `Refused(mcp_session_unavailable)` |
| — | the tool is not listed, or its `visibility` excludes `app` | — | `Refused(mcp_tool_not_for_app)` |
| — | arguments past 32 KiB | — | `Refused(mcp_request_too_large)` |
| — | arguments that are not one JSON object | — | `Refused(invalid_request)` |
| — | admitted, not destructive | Sending | `Admitted` |
| — | admitted, destructive (`readOnlyHint` not true and `destructiveHint` not false) | Waiting | `ApprovalRequested`, then the review is shown |
| — | admitted, destructive, 16 reviews already open | — | `Withdrawn(RequestCancelled)` by the system; `temporarily_unavailable` |
| — | admitted, destructive, the conversation already ended | — | `Withdrawn(ConversationEnded)`; `mcp_cancelled` |
| Waiting | the person allows | Sending | `Approved`, by that person and their request |
| Waiting | the person denies, or cancels the review | — | `Denied`, by that person; `mcp_approval_denied` |
| Waiting | 5 minutes with no answer | — | `Expired`, by the system; `mcp_approval_expired` |
| Waiting | the caller goes (its socket closes) | — | `Withdrawn(RequestCancelled)`, by the app; `mcp_cancelled` |
| Waiting | `mcp.releaseApp` for its mount | — | `Withdrawn(AppTornDown)`, by the app; `mcp_cancelled` |
| Waiting | the conversation closes, is deleted or stopped | — | `Withdrawn(ConversationEnded)`, by the system; `mcp_cancelled` |
| Waiting | an answer and the deadline at once | — | whichever ended the review first; an answer is never lost to the expiry |
| Sending | the server answers within 56 KiB | — | `Completed(Answered{isError, bytes})`; the answer verbatim |
| Sending | past 56 KiB | — | `Completed(Failed(mcp_result_too_large))` |
| Sending | a JSON-RPC error | — | `Completed(Failed(mcp_remote_error))`; its code and message as details |
| Sending | an answer that is no MCP answer | — | `Completed(Failed(mcp_remote_error))`, no details |
| Sending | no answer in 60 s | — | `Completed(Failed(mcp_timed_out))` |
| Sending | the session ends | — | `Completed(Failed(mcp_session_unavailable))` |
| Sending | the caller goes | Sending | the call finishes on its own task and is recorded; the answer goes nowhere |
| any | a record cannot be written | — | `audit_unavailable`; the step it would have recorded is not taken |

### A review

Its answer: `conversation.answer` with `allow` or `deny`, or
`conversation.cancel` (a denial). An answer to a review that has ended, or
with an option it did not offer, or naming another execution, is
`stale_permission` and changes nothing. Review identities are `app-<UUID>`, so
none is taken for an agent's.

### A resource read

| Event | Effect, and what is recorded |
| --- | --- |
| refused as a tool call is (app, server, session) | the same codes |
| a URI that is no `ui://` resource | `Refused(invalid_request)` |
| read, and not an app's HTML | `Completed(Failed(mcp_app_unknown))` |
| read | `Completed(Answered)`, then `TicketIssued{digest, size, sha256}`; the ticket answered |
| no room to hold it: 16 MiB or 64 tickets per conversation | `Completed(Failed(temporarily_unavailable))` |
| the issue cannot be recorded | the ticket is discarded, unreported; `audit_unavailable` |

### A ticket

| Event | Effect, and what is recorded |
| --- | --- |
| `GET /mcp-resources` with it, within 60 s | `TicketRedeemed`, by the app, recorded before the bytes are served |
| redeemed, and that cannot be recorded | `503`, nothing served, the ticket spent |
| redeemed again, expired, released, never issued | the same empty `404` |
| 60 s pass | `TicketExpired`, by the system |
| its mount released, or its conversation ended | `TicketExpired`, by the system |

## Lanes

App calls have 4 slots on each socket. A destructive call holds its slot while
it waits, so held calls can fill a socket's lane, but never the lanes
`conversation.read` and `conversation.answer` use. `mcp.releaseApp` is a
control. A socket that goes aborts its app calls; each call's own task then
withdraws its review on record, or finishes a call already sent. The 32
gateway-wide slots are held by those tasks, not by the socket, so a caller
that reconnects cannot start calls past them.

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
