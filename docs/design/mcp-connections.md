# One MCP connection per server for each harness session, owned by the gateway

Design for #346, under [ADR 344](../adr/todo/344-mcp-ui.md). The ADR decides
that the gateway owns the connection to each configured MCP server and hands
the harness a stand-in. This document says how, and writes down the states
and orderings the tests come from ([gate 15](../../CODING_STANDARDS.md#gates)).

## What is built

```text
harness session ──spawns──▶ nessa mcp-relay <socket> <server> <configuration>
                                 │  (bytes, after a one-line hello)
                                 ▼
gateway  relay listener ─▶ McpServers::open (SDK) ─▶ McpSession: one server
                                                     process, one connection
                                                        │  newline JSON-RPC
                                                        ▼
                                              that server process (its group)
gateway  conversation view ◀── tool UI lookup ── the open sessions' tool lists
```

Arrows are calls and bytes, not ownership. Each stand-in — one harness
session's MCP client for one server — gets a session of its own: its own
server process and the one connection to it, opened when the stand-in says
hello and closed when the stand-in ends. Two conversations never share a
server process, so neither blocks, sees, or outlives the other's, and Nessa's
own `nessa` shell server runs per session as it did. The SDK
(`crates/nessa-sdk/src/infrastructure/mcp/`) owns the protocol: the
handshake, `tools/list` with each tool's `_meta.ui`, `resources/read` of an
MCP App resource, forwarding a stand-in's traffic, and each session's
process. The gateway (`crates/nessa-server/src/mcp_servers/`) owns the
socket, the `mcp-relay` command, and the lookup the conversation view asks.

Which conversation a stand-in belongs to is not known yet: what a harness is
launched with is per agent, not per conversation. #348 adds it — a session
token in `session/new` and `session/load`, carried in the hello — and keys
each session by its conversation before any `mcp.callTool` or
`mcp.readResource` route exists.

- **Handshake.** The gateway sends `initialize` with protocol `2025-06-18`
  and `capabilities.extensions: { "io.modelcontextprotocol/ui": { mimeTypes:
  ["text/html;profile=mcp-app"] } }`, accepts a server answering `2025-06-18`,
  `2025-03-26` or `2024-11-05`, then sends `notifications/initialized`; the
  session is open then, and its tools are listed in the background. It
  declares no `roots`, `sampling` or `elicitation`.
- **What the harness is given.** In `session/new`, each configured server is
  replaced by a stand-in under the same name: the gateway's own executable,
  `mcp-relay`, the relay socket, the server's name, and a digest of the
  server's configured command and arguments.
- **Stand-in traffic.** The harness's `initialize` is answered by the gateway
  from the upstream's own answer (its protocol version, capabilities, server
  information and instructions), less `resources.subscribe`;
  `notifications/initialized` is not forwarded, since the upstream was
  initialized once. `resources/subscribe` and `resources/unsubscribe` are
  refused (`-32601`): their updates are not routed back. Every other request
  is forwarded with a fresh upstream id and answered with the harness's own
  id, verbatim — except that a `tools/list` answer leaves out a tool whose
  `_meta.ui.visibility` excludes `model`, and a `tools/call` naming a tool
  the latest list this stand-in forwarded left out is refused (`-32602`):
  having declared MCP Apps, the host keeps an app's own tools from the model.
  `notifications/cancelled` for a forwarded request cancels it upstream. A
  server's `*/list_changed` notifications are passed on; its other
  notifications (progress, logging) are not. A server's own requests are
  answered by the gateway: `ping` with `{}`, anything else with `-32601`,
  since the gateway declared none of them.
- **Tool UI.** `tools/list` (paged by `nextCursor`) gives each tool's
  `_meta.ui`: `resourceUri` (a `ui://` URI) and `visibility` (`model`, `app`;
  both when absent). A tool whose `_meta.ui` cannot be read is kept without a
  UI; a tool whose name cannot be one is left out. A session's list is read
  when it opens and again on `notifications/tools/list_changed`.
- **UI resources.** `resources/read` of a `ui://` URI must answer one content
  for that URI with MIME `text/html;profile=mcp-app` and `text` (or `blob`
  holding UTF-8), with its `_meta.ui`: `csp` (`connectDomains`,
  `resourceDomains`, `frameDomains`, `baseUriDomains`), `permissions`
  (`camera`, `microphone`, `geolocation`, `clipboardWrite`, each asked for as
  `{}` or `true`; anything else, and any other permission, is never granted),
  `domain`, `prefersBorder`.
- **The view.** `ConversationTool.mcp.resourceUri` is filled when the view
  is read, from the open sessions of the call's server, for the one listed
  tool the call names (`McpTool::names`: exactly, or in Claude's replaced
  spelling; two candidates give none). It is filled only when at least one
  open session has listed its tools and every one that has agrees on that
  tool's UI; otherwise none is attached — which session the call went through
  is not known until #348 — and a disagreement is logged. The view's revision
  takes the filled URIs into account, so a window holding the same revision
  holds the same URIs. The desktop maps a gateway tool to a `widget` part with
  `toolWidget`.

## Bounds

| What | Bound | Past it |
| --- | --- | --- |
| A frame from the server, or from a stand-in | 16 MiB | the session (or the stand-in) ends: `TooLarge` |
| The stand-in's hello line | 4 KiB, 5 s | the stand-in is closed without an answer |
| The gateway's answer to a hello | 35 s (opening is 30 s at most) | the relay exits 1 |
| Requests in flight on one connection | 256 | that request fails `Busy`; forwarded: a JSON-RPC error |
| `tools/list` | 32 pages, 1024 tools | `TooLarge` |
| A `ui://` URI | 2048 bytes | the tool has no UI; a read cannot be asked (`UiResourceUri` refuses it) |
| A UI resource's HTML | 4 MiB | `TooLarge` |
| CSP sources, the app's `domain` | 64 per list, 512 bytes each | `TooLarge` |
| Opening a session: spawn to `initialize` answer | 30 s | `Timeout`, the process group is killed |
| The gateway's own `tools/list`, `resources/read` | 10 s each | `Timeout`; the connection stays |
| A forwarded request | none from the gateway | the harness's own deadline and cancellation |
| Closing a session | stdin closed, 2 s, then the process group killed | — |
| A refusal's message | 512 characters, control characters as spaces | — |

## Failures are typed

`McpError`: `InvalidConfiguration` (servers that cannot be launched as
configured), `NotConfigured` (no such server), `Start` (could not be
launched), `Handshake` (refused or unreadable `initialize`), `Timeout`,
`ServerGone` (the process ended or its pipes closed), `Remote { code,
message }` (a JSON-RPC error), `Malformed` (an answer of the wrong shape),
`TooLarge`, `NotAnApp` (not `text/html;profile=mcp-app`), `Busy`, `Stopped`
(the gateway is shutting down), `Closed` (the session's harness session
ended).

## States

### A session

One per stand-in: one server process (on Unix, the leader of a process group
of its own) and the one connection to it.

| State | Event | Next | Effect |
| --- | --- | --- | --- |
| — | a stand-in says hello | Opening | launch the process, send `initialize` |
| Opening | `initialize` answered, supported version | Open | `notifications/initialized`; registered; the stand-in is accepted; `tools/list` in the background (its failure leaves the list as it was, logged) |
| Opening | refused, unsupported version, unreadable answer (something on stdout that is not one) | — | `Handshake`; the process group is killed; the stand-in is refused `unavailable` |
| Opening | no answer within 30 s | — | `Timeout`; as above |
| Opening | process cannot be launched | — | `Start`; the stand-in is refused `unavailable` |
| Opening | process exits | — | `ServerGone`; as above |
| Opening | gateway stops | — | `Stopped`; registration and stop share one lock, so either stop closes it or the opening sees the stop and closes it itself |
| Open | the stand-in ends: the harness closes, its socket breaks, the relay is killed, a frame that is not JSON or is past the bound | Closed | calls waiting end `Closed`, unanswered (closing the server's stdin ends them; no `notifications/cancelled` is sent first); 2 s; the process group killed |
| Open | process exits, stdout closes, oversize or non-JSON frame | Gone | calls waiting end (`ServerGone`, `TooLarge`, `Malformed`); the stand-in is closed, and its relay exits; the process group killed |
| Open | gateway stops | Closed | as the stand-in ending, with `Stopped` |
| Open | the last handle to it is dropped without closing | — | the process group killed |

Nothing restarts a session. A harness whose server ended starts it again the
way it would have when it owned the process — usually with a new session of
its own, and so a new stand-in.

### A gateway request (list, read)

| Order | Outcome |
| --- | --- |
| answer before 10 s | its result, checked against the bounds |
| no answer by 10 s | `Timeout`; `notifications/cancelled` sent; a late answer is dropped |
| a JSON-RPC error answer | `Remote { code, message }` |
| the session ends first | its end cause |
| answer and session end in one poll | the answer, if it was read before the end was observed (one reader orders them) |
| 256 already in flight | `Busy`, nothing sent |
| two lists of one session finish out of order | the one asked later is kept |

### A stand-in

| State | Event | Next | Effect |
| --- | --- | --- | --- |
| Hello | a line within 4 KiB and 5 s naming a configured server and its current configuration | Opening | a session of its own (above) |
| Hello | unknown server | Closed | refused `unknown-server` |
| Hello | configuration digest differs | Closed | refused `configuration-changed` |
| Hello | oversize, unreadable, or no line in 5 s | Closed | closed without an answer |
| Opening | session open | Serving | `accepted` |
| Opening | opening fails | Closed | refused `unavailable` with the typed reason; the relay exits 1, so the harness sees its server fail to start |
| Serving | harness `initialize` | Serving | answered from the upstream's `initialize` result, less `resources.subscribe` |
| Serving | harness request | Serving | forwarded; the answer comes back with the harness's id |
| Serving | `tools/list` answer | Serving | forwarded without the tools whose visibility excludes the model; the tools the latest list showed are callable again |
| Serving | `tools/call` for a tool the latest list left out | Serving | refused `-32602`, nothing forwarded |
| Serving | `resources/subscribe`, `resources/unsubscribe` | Serving | refused `-32601`, nothing forwarded |
| Serving | a request reusing the id of one still waiting | Serving | refused `-32600`, nothing forwarded |
| Serving | harness `notifications/cancelled` for a forwarded request | Serving | that request is dropped; upstream `notifications/cancelled` |
| Serving | harness reuses a cancelled request's id while the old call's answer is in flight | Serving | the old answer is dropped; the new request gets its own |
| Serving | harness closes, or its socket breaks | Closed | the session is closed (above) |
| Serving | session ends | Closed | the socket is closed, and the `mcp-relay` process exits at once, its stdin unread: the harness sees its server end, as when it owned the process |

The relay socket: a socket at the path that a gateway still listens on fails
the bind, and MCP servers are off for that run; a socket nothing listens on is
replaced.

## What one connection per harness session means

- **Context fingerprint.** The SDK fingerprints what the harness is launched
  with, and the harness is now launched with the stand-in. Its arguments carry
  the server's name and a digest of the configured command and arguments, so
  the fingerprint still changes exactly when a configured server does; it also
  changes when the gateway's executable or the relay socket's path moves (the
  socket is `/tmp/nessa-mcp-<uid>/<digest of the namespace's path>.sock`, so it
  does not move between runs). The SDK's fingerprint is unchanged code.
- **Restarts.** A gateway restart ends every session with the agents. A
  restored conversation's harness opens new sessions through its stand-ins; a
  handle an old session gave out is unknown to the new one, and the server says
  so — as it did when the harness owned the process.
- **A server that exits** ends its session and its stand-in, and the relay
  exits, so its harness sees its server end, as when it owned the process.
  Other conversations' sessions of the same server are untouched.
- **What a server is started with.** The workspace as its working directory,
  and `HOME`, `USER`, `LOGNAME`, `TMPDIR`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`
  and the agents' search path from the gateway's own environment; its standard
  error goes to the gateway's. It used to be whatever its harness gave it.
- **Who can connect.** The relay socket is in a `0700` directory owned by the
  gateway's user (`/tmp/nessa-mcp-<uid>`, short because a socket's path has a
  platform limit — 104 bytes on macOS — that a namespace under a long data
  directory passes); any process of that user can open a session through it,
  as it could start the server itself. A directory there that is not private
  to the user, or anything there that is not a socket, leaves MCP servers off
  for the run, logged; they are never handed to an agent directly.

## Tests

Each row above has at least one test, named after it:

- SDK, in-process fixture servers over in-memory pipes and a manual clock
  (`crates/nessa-sdk/tests/infrastructure/mcp/`): the handshake and declared
  extension, version refusal, list with pages and `_meta.ui`, a tool without
  UI, unreadable `_meta.ui`, list bounds and order, read of an app resource,
  wrong MIME, oversize HTML and frame, `Timeout` and the late answer,
  `Remote`, `Busy`, the stand-in's `initialize`, forwarding with id rewriting,
  cancellation both ways, id reuse, hidden tools, subscriptions, server
  requests answered, list-changed notices, sessions apart from each other,
  the view's lookup agreeing, disagreeing and not yet listed, stop racing an
  opening.
- SDK, real processes (python fixture): launching as configured, separate
  processes per session, a long call in one not blocking another, killing one
  leaving another, closing stopping the process group, the session ending
  with its stand-in, and the **stateful** fixture: a handle returned to a
  stand-in's call resolves in a later read on the same session, and not on
  another.
- Gateway: hello refusals, the relay command's exit, the stand-ins and digest
  in `session/new`, a relay process killed outright ending its server's
  process group, a relay exiting when its server ends with its stdin still
  open, the view's `resourceUri` and revision, the schema bound.
- Desktop: a gateway tool with a `resourceUri` maps to a `widget` part.
- Live: `scripts/mcp-test-server/live-check.mjs` with Claude and Codex.
