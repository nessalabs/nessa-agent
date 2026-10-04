# Gateway protocol

The gateway serves authenticated WebSocket sessions at `/session`. It sends
`session.challenge`, accepts `session.authenticate`, and returns verified session
metadata. Every product command checks current credentials, membership, and policy.
HTTP `/health` reports process liveness only.

| Source | Purpose |
| --- | --- |
| [product/manifest.json](product/manifest.json) | Authenticated method and event catalog |
| [product/v1.json](product/v1.json) | Session, credential, and termination payloads |
| [manifest.json](manifest.json) | Shared health method schema |
| [schemas/v1/](schemas/v1/) | Shared payloads, frames, and shortcut documents |
| [defaults/](defaults/) | Bundled shortcut defaults, and the stage → gateway port table |
| [fixtures/v1/](fixtures/v1/) | Validated shared wire examples |

```sh
pnpm protocol:generate
pnpm protocol:check
```

Edit source schemas and manifests together with callers, fixtures, and handlers.
Generated TypeScript and Rust files name their source in their headers. No protocol
or schema version bump is needed merely to change this repository's current contract.

| Name | Purpose |
| --- | --- |
| `session.challenge` | Per-socket nonce and accepted protocol range |
| `session.authenticate` | Credential proof and nonce; returns verified session |
| `auth.session` | Current authenticated metadata |
| `server.health` | Authorized health read (`server.read`) |
| `conversation.create`, `conversation.read`, `conversation.send`, `conversation.steer` | Conversation creation, projection reads, and input submission |
| `conversation.list` | The caller's conversations, newest first, with title and last line said, at most 500, and `complete` saying whether that is all of them; archived ones only on request; opens no provider |
| `conversation.archive`, `conversation.unarchive`, `conversation.delete` | Hide or restore a conversation in the list; delete permanently (history, uploads and summary erased, audit kept, identity never reused) |
| `conversation.remove`, `conversation.reorder`, `conversation.answer`, `conversation.cancel`, `conversation.close` | Pending-work, permission, and lifecycle controls |
| `credential.issue`, `credential.list`, `credential.revoke` | Credential administration (`credential.manage`) |
| `pairing.create`, `pairing.pending`, `pairing.status`, `pairing.approve`, `pairing.deny`, `pairing.cancel` | Owner side of native device pairing (`credential.manage`, then Auth's exact consent check): a one-time code, the unfinished enrollments, one enrollment, and approval of the exact claimed key, denial or cancellation. `pairing_not_configured` unless `config.json` names a native listen address. Refusals are a `PairingErrorCode` or the session's own codes. See [device pairing](../docs/design/auth/device-pairing.md#owner-routes-and-mounting-slice-2a) |
| `mcpServers.list`, `mcpServers.save`, `mcpServers.remove`, `mcpServers.inspect` | The gateway's configured MCP servers (`credential.manage`, refused `forbidden` before params are read): list them with variable names only, save one (`kind: "stdio"`, `env` values, `null` keeping a stored one, `value` required) or remove one, naming the listed `revision`; or inspect a saved one — started once outside any conversation, its tools listed with their hints and each MCP App's CSP and permissions, then stopped — within `x-mcpServerInspect` (`MCP_SERVER_INSPECT_*` in Rust, `mcpServerInspect` in TypeScript). Each change rewrites the whole of `config.json` under its lock, is audited, and reaches the next conversation; each inspection is audited. `mcp_servers_not_configured` where the gateway holds no live MCP server set. Refusals are an `McpServersErrorCode`. See [MCP connections](../docs/design/mcp-connections.md#managing-the-stored-servers) |
| `attachment.begin` | Single-use ticket to upload one file into a conversation (`conversation.write`). The bytes travel on `PUT /attachments`, never in a socket message; its answer is the reference a message uses |

Frames use `req`, `res`, and `event`. A transport `id` correlates a response with
its request. Mutations separately carry a stable `requestId` for explicit retries.
Credential and session `expiresAt` may be null; issuance defaults to no expiry.
Authentication challenges advertise a Unix-second deadline rounded up from
millisecond wall time. A single monotonic timeout covers challenge delivery and
authentication; expiry closes with retryable `handshake_timeout` (4006). Typed close reasons distinguish
terminal authority failures from retryable transport or dependency failures.

A conversation command can open or restore an agent, which the gateway spends a
real budget on before it can answer at all.
[defaults/agent-startup-budgets.json](defaults/agent-startup-budgets.json) is
the one table for that: the gateway compiles it into what it spends, and the
client into how long it waits. A client that gave up first deleted its request
and dropped the typed answer when it arrived, so the caller learned nothing
about a failure the gateway had described exactly.

A conversation command the gateway dispatched and refused answers with a
`ConversationErrorCode`. The typed code is the contract; the message text is not.
Access and routing failures are answered by the session before a conversation
command is dispatched and carry their own codes, so this is not every code a
conversation request can receive. `agent_startup_deadline` means the agent was
still starting when its budget expired, so nothing reached the provider and the
same command is safe to repeat. `agent_not_configured` and `invalid_request`
reject the command until their cause is addressed.

## An MCP App's calls

An MCP App (ADR 344) reaches its own server through two methods, and its
host releases it through a third. Each names
its conversation, the app — the tool call whose UI it is (`McpAppReference`:
`executionId`, `toolId`) — and the server. Their shapes are
`McpCallToolParams` / `McpCallToolResult` and `McpReadResourceParams` /
`McpReadResourceResult` in [product/v1.json](product/v1.json).

- **`mcp.callTool`** calls a tool the conversation's own session last listed
  with `visibility` including `app`. `argumentsJson` is at most 32 KiB, and
  is sent as parsed: re-encoded, a duplicate key's last value kept.
  `resultJson` is the server's `CallToolResult`, re-encoded, at most 56 KiB
  measured as the JSON string the response carries it in.
  `isError: true` is a result, not a refusal.
- **Destructive tools** wait for approval first. A tool is destructive when
  `readOnlyHint` is not true and `destructiveHint` is not false, so a tool with
  no annotations waits. The approval is a review in the conversation's
  `permissions`, with `origin: {kind: "app", server, tool}`, whatever the
  approval mode. It shows the arguments as they will be sent. It is answered
  with `conversation.answer` or `conversation.cancel`. A conversation's open
  app reviews take at most 16 000 bytes of its view together, 16 at most: a
  review past that alone is refused `mcp_request_too_large`, and one that
  does not fit beside those open is refused `temporarily_unavailable`.
  Allowed, the call is made only if the tool is still listed as it was.
- **A waiting call stays pending** until the person answers, or the review
  expires (`reviewDeadlineMs`, below; `mcp_approval_expired`), or it is
  withdrawn (`mcp_cancelled`). It is withdrawn when the request is cancelled —
  its socket closes — the app is torn down (`mcp.releaseApp`), or the
  conversation ends. A client that stops waiting withdraws nothing: the
  review stays open, and the call is still made if the person allows it.
- **How long a call can take is published once**, as `x-mcpAppCallTiming`:
  `reviewDeadlineMs` for a review to be answered, then `callTimeoutMs` for
  the server to answer a call (`mcp_timed_out`); `readTimeoutMs` for it to
  answer a resource read; and `clientAllowanceMs` for opening the
  conversation and recording each step. The schema holds their values;
  nothing here repeats them. A client waits at least
  `reviewDeadlineMs + callTimeoutMs + clientAllowanceMs` (`callDeadlineMs`)
  before giving up on `mcp.callTool`, and as long on `mcp.readResource`,
  which may open the conversation first and never outlasts a call. The
  gateway, the SDK's caller and the client read the generated values
  (`MCP_APP_REVIEW_DEADLINE_MS`, `MCP_APP_CALL_TIMEOUT_MS` and
  `MCP_APP_READ_TIMEOUT_MS` in Rust, `mcpAppCallTiming` in TypeScript); no
  layer writes its own.
- **`mcp.readResource`** reads a resource of the app's server once and holds
  exactly those bytes. Its answer says what they are (`mimeType`, `size`,
  `sha256`, the app's `csp`, `permissions`, `domain`, `prefersBorder`) and
  gives a `ticket`. The bytes never travel on the socket. A resource whose
  URI, `csp` and `domain` take more than 48 KiB encoded is refused
  `mcp_result_too_large`: no answer could carry them.
- **App calls have a lane of their own**, 4 at once per socket, and 32
  running at once on the gateway, each counted until it ends rather than
  until its socket goes. Past either they are refused
  `temporarily_unavailable`, so held calls never stop `conversation.read` or
  `conversation.answer`.
- **`mcp.releaseApp`** says the host tore one mount of an app down. Each app
  reference carries the host's own `instanceId` for its mount, since one tool
  call can be mounted more than once. The release withdraws that mount's
  open reviews (their calls answer `mcp_cancelled`) and releases its
  resource tickets, and is idempotent. Nothing is admitted, opened or issued
  for that mount again — across a close and a reopening, and when the
  release came before the conversation was open: its later calls answer
  `mcp_cancelled`. An `mcp.callTool` already handed to the session is not
  stopped: it may still reach the server, and answers with its own outcome. It travels on the
  control lane, never the app lane, so held calls can never stop an app
  being released.
- **What a refusal tells the host.** Nothing reached the server for
  `mcp_app_unknown`, `mcp_server_mismatch`, `mcp_tool_not_for_app`,
  `mcp_request_too_large`, `mcp_approval_denied`, `mcp_approval_expired`,
  `mcp_cancelled` or `invalid_request`; an
  `mcp_app_unknown` for a resource that is not an app's HTML, or an
  `mcp_cancelled` for one whose mount or conversation ended while it was
  read, was read, which changes nothing. The server may have been asked for
  `mcp_session_unavailable`, `mcp_timed_out`, `mcp_remote_error` (its JSON-RPC
  error in `McpRemoteErrorDetails`, or no details for an answer that is no
  MCP answer, or a code past what a JSON number keeps) or
  `mcp_result_too_large`. `temporarily_unavailable` may come either side:
  a lane, slot or review that was full, or a busy session, asked nothing; a
  resource read that found no room, or no ticket, to hold its bytes was read;
  and a call whose task failed may have been sent.
  `audit_unavailable` says a step could not be
  recorded and was not taken — except the last: a call already made whose
  answer could not be recorded is answered `audit_unavailable` too, its
  answer withheld.

`GET /mcp-resources`, on the gateway's HTTP listener, serves a held resource,
as `PUT /attachments` takes an upload:
- **Redeeming.** The ticket goes in the `x-nessa-resource-ticket` header,
  never in the URL, and is the whole authority: 256 random bits, single use,
  valid for at most `expiresInMs` from its issue, bound to its conversation and app, and issued only after
  the socket's policy and audit. The route authenticates nobody else, and has
  the same origin checks and CORS as `/attachments`.
- **Refusals.** An unknown, used, expired or wrong-credential ticket gets the
  same `404` with no body.
- **Unrecorded.** A redemption is audited before anything is served. When
  that record cannot be written the answer is `503` with no body: the ticket
  is spent and nothing is served. Read the resource again for a new one.
- **The response.** `Content-Type: text/html;profile=mcp-app`,
  `X-Content-Type-Options: nosniff`, `Cache-Control: no-store` and
  `Content-Disposition: attachment`.
- **The host's job.** It fetches the bytes, checks their SHA-256 against
  `sha256` before rendering, and hands them to its sandbox; the frame never
  sees the ticket.

Credential lifecycle RPC errors distinguish `credential_conflict`,
`credential_capacity`, and `credential_not_found` from
`credential_store_unavailable`. The first three reject the command; they do not
signal a transient connection failure. Empty issuance grants are invalid.

See the [authentication decision](../docs/adr/done/0010-local-authentication.md),
[local setup guide](../docs/guides/local-auth.md), and
[SDK guide](../packages/nessa-client/README.md).
