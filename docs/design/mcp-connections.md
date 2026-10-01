# One MCP connection per server, owned by the gateway

Design for #346, under [ADR 344](../adr/todo/344-mcp-ui.md). The ADR decides
that the gateway owns the one connection to each configured MCP server and
hands the harness a stand-in. This document says how, and writes down the
states and orderings the tests come from ([gate 15](../../CODING_STANDARDS.md#gates)).

## What is built

```text
harness ──spawns──▶ nessa mcp-relay <socket> <server> <configuration>
                         │  (bytes, after a one-line hello)
                         ▼
gateway  relay listener ─▶ McpServers (SDK) ─▶ one McpConnection per server
                                                  │  newline JSON-RPC
                                                  ▼
                                           the configured server process
gateway  conversation view ◀── tool UI lookup ─┘  (cached tools/list)
```

Arrows are calls and bytes, not ownership. The SDK
(`crates/nessa-sdk/src/infrastructure/mcp/`) owns the protocol: the
handshake, `tools/list` with each tool's `_meta.ui`, `resources/read` of an
MCP App resource, forwarding a stand-in's traffic, and each server's
lifecycle. The gateway (`crates/nessa-server/src/mcp_servers/`) owns the
socket, the `mcp-relay` command, and the lookup the conversation view asks.

- **Handshake.** The gateway sends `initialize` with protocol `2025-06-18`
  and `capabilities.extensions: { "io.modelcontextprotocol/ui": { mimeTypes:
  ["text/html;profile=mcp-app"] } }`, accepts a server answering `2025-06-18`,
  `2025-03-26` or `2024-11-05`, then sends `notifications/initialized` and
  lists the tools. It declares no `roots`, `sampling` or `elicitation`.
- **What the harness is given.** In `session/new`, each configured server is
  replaced by a stand-in under the same name: the gateway's own executable,
  `mcp-relay`, the relay socket, the server's name, and a digest of the
  server's configured command and arguments.
- **Stand-in traffic.** The harness's `initialize` is answered by the gateway
  from the upstream's own answer (its protocol version, capabilities, server
  information and instructions); `notifications/initialized` is not
  forwarded, since the upstream was initialized once. Every other request is
  forwarded with a fresh upstream id and answered with the harness's own id,
  verbatim. `notifications/cancelled` for a forwarded request cancels it
  upstream. A server's `*/list_changed` notifications go to every connected
  stand-in; its other notifications (progress, logging) are not forwarded.
  A server's own requests are answered by the gateway: `ping` with `{}`,
  anything else with `-32601`, since the gateway declared none of them.
- **Tool UI.** `tools/list` (paged by `nextCursor`) gives each tool's
  `_meta.ui`: `resourceUri` (a `ui://` URI) and `visibility` (`model`, `app`;
  both when absent). A tool whose `_meta.ui` cannot be read is kept without a
  UI; a tool whose name cannot be one is left out. The list is read after the
  handshake and again on `notifications/tools/list_changed`.
- **UI resources.** `resources/read` of a `ui://` URI must answer one content
  for that URI with MIME `text/html;profile=mcp-app` and `text` (or `blob`
  holding UTF-8), with its `_meta.ui`: `csp` (`connectDomains`,
  `resourceDomains`, `frameDomains`, `baseUriDomains`), `permissions`
  (`camera`, `microphone`, `geolocation`, `clipboardWrite`; others are
  dropped, never granted), `domain`, `prefersBorder`.
- **The view.** `ConversationTool.mcp.resourceUri` is filled when the view
  is read, from the cached list, for the one listed tool the call names
  (`McpTool::names`: exactly, or in Claude's replaced spelling; two candidates
  give none). The view's revision takes the filled URIs into account, so a
  window holding the same revision holds the same URIs. The desktop maps a
  gateway tool to a `widget` part with `toolWidget`.

## Bounds

| What | Bound | Past it |
| --- | --- | --- |
| A frame from the server, or from a stand-in | 16 MiB | the connection (or the stand-in) ends: `TooLarge` |
| The stand-in's hello line | 4 KiB, 5 s | the stand-in is refused and closed |
| Requests in flight on one connection | 256 | that request fails `Busy`; forwarded: a JSON-RPC error |
| `tools/list` | 32 pages, 1024 tools | `TooLarge` |
| A `ui://` URI | 2048 bytes | the tool has no UI; a read cannot be asked (`UiResourceUri` refuses it) |
| A UI resource's HTML | 4 MiB | `TooLarge` |
| CSP sources, the app's `domain` | 64 per list, 512 bytes each | `TooLarge` |
| `initialize` (spawn to answer) | 30 s | `Timeout`, the process is stopped |
| The gateway's own `tools/list`, `resources/read` | 10 s each | `Timeout`; the connection stays |
| A forwarded request | none from the gateway | the harness's own deadline and cancellation |
| Stopping a server | stdin closed, 2 s, then killed | — |

## Failures are typed

`McpError`: `InvalidConfiguration` (servers that cannot be launched as configured), `NotConfigured` (no such server), `Start` (could not be
launched), `Handshake` (refused or unreadable `initialize`), `Timeout`,
`ServerGone` (the process ended or its pipes closed), `Remote { code,
message }` (a JSON-RPC error), `Malformed` (an answer of the wrong shape),
`TooLarge`, `NotAnApp` (not `text/html;profile=mcp-app`),
`Busy`, `Stopped` (the gateway is shutting down).

## States

### A server's connection

One slot per configured server. A **generation** is one process and its one
connection; everything attached to it (stand-ins, the tool list, in-flight
requests) ends with it.

| State | Event | Next | Effect |
| --- | --- | --- | --- |
| Idle | use: gateway start, a stand-in connects, a list or read | Starting | launch the process, send `initialize` |
| Starting | another use | Starting | waits for the same start; no second process |
| Starting | `initialize` answered, supported version | Ready | `notifications/initialized`, then `tools/list` (its failure leaves the list empty and is logged) |
| Starting | refused, unsupported version, unreadable answer | Idle | `Handshake` to every waiter; process stopped |
| Starting | no answer within 30 s | Idle | `Timeout` to every waiter; process stopped |
| Starting | process cannot be launched | Idle | `Start` to every waiter |
| Starting | process exits | Idle | `ServerGone` to every waiter |
| Ready | use | Ready | served on this generation |
| Ready | process exits, stdout closes, oversize or non-JSON frame | Gone | pending gateway requests fail `ServerGone` (`TooLarge` for an oversize frame); every stand-in of this generation is closed |
| Gone | use | Starting | a new generation: new process, new upstream session |
| any | gateway stops | Stopped | waiters fail `Stopped`; stand-ins closed; stdin closed, 2 s, kill |
| Stopped | use | Stopped | `Stopped` |

A start is never retried by itself: the next use starts again. A server that
keeps exiting costs one launch per use, each bounded by the 30 s budget.

### A gateway request (list, read)

| Order | Outcome |
| --- | --- |
| answer before 10 s | its result, checked against the bounds |
| no answer by 10 s | `Timeout`; `notifications/cancelled` sent; a late answer is dropped |
| a JSON-RPC error answer | `Remote { code, message }` |
| the generation ends first | `ServerGone` |
| answer and generation end in one poll | the answer, if it was read before the end was observed (one reader orders them) |
| 256 already in flight | `Busy`, nothing sent |

### A stand-in

| State | Event | Next | Effect |
| --- | --- | --- | --- |
| Hello | a line within 4 KiB and 5 s naming a configured server and its current configuration | Attaching | start or join the server's generation |
| Hello | unknown server | Closed | refused `unknown-server` |
| Hello | configuration digest differs | Closed | refused `configuration-changed` |
| Hello | oversize, unreadable, or no line in 5 s | Closed | closed without an answer |
| Attaching | generation Ready | Serving | `accepted` |
| Attaching | start fails | Closed | refused `unavailable` with the typed reason; the relay exits 1, so the harness sees its server fail to start |
| Serving | harness `initialize` | Serving | answered from the upstream's `initialize` result |
| Serving | harness request | Serving | forwarded; the answer comes back with the harness's id |
| Serving | harness `notifications/cancelled` for a forwarded request | Serving | that request is dropped; upstream `notifications/cancelled` |
| Serving | harness closes | Closed | its in-flight requests are cancelled upstream |
| Serving | generation ends | Closed | the stand-in is closed: the harness sees its server end, as when it owned the process |
| Serving | oversize or non-JSON frame from the harness | Closed | as above |

## What one connection means

- **Context fingerprint.** The SDK fingerprints what the harness is launched
  with, and the harness is now launched with the stand-in. Its arguments carry
  the server's name and a digest of the configured command and arguments, so
  the fingerprint still changes exactly when a configured server does; it also
  changes when the gateway's executable or the relay socket's path moves (the
  socket is `/tmp/nessa-mcp-<uid>/<digest of the namespace's path>.sock`, so it
  does not move between runs). The SDK's fingerprint is unchanged code.
- **Restarts.** A gateway restart starts every server again: a new upstream
  session. A restored conversation's harness reconnects through its stand-in to
  the new one; a handle the old session gave out is unknown to it, and the
  server says so.
- **A server that exits** takes its generation's stand-ins with it, so each
  harness sees its server end, as it did when it owned the process. Gateway
  requests in flight fail `ServerGone`. The next use starts a fresh server, and
  only harness sessions started after that reach it.
- **Shared by every conversation.** One upstream session serves every
  conversation's agent and, later, every app (#348). A server that keeps state
  per session keeps it across conversations now.
- **What the server is started with.** The workspace as its working directory,
  and `HOME`, `USER`, `LOGNAME`, `TMPDIR`, `LANG`, `LC_ALL` and the agents'
  search path from the gateway's own environment; its standard error goes to
  the gateway's. It used to be whatever its harness gave it.
- **Who can connect.** The relay socket is in a `0700` directory owned by the
  gateway's user (`/tmp/nessa-mcp-<uid>`, short because a socket's path has a
  platform limit — 104 bytes on macOS — that a namespace under a long data
  directory passes); any process of that user can reach the server through it,
  as it could start the server itself. A directory there that is not private
  to the user, or anything there that is not a socket, leaves MCP servers off
  for the run, logged; they are never handed to an agent directly.

## Tests

Each row above has at least one test, named after it:

- SDK, in-process fixture server over a duplex pipe and a manual clock
  (`crates/nessa-sdk/tests/infrastructure/mcp/`): handshake and declared
  extension, version refusal, list with pages and `_meta.ui`, a tool without
  UI, unreadable `_meta.ui`, list bounds, read of an app resource, wrong MIME,
  not `ui://`, oversize HTML and frame, `Timeout` and the late answer,
  `Remote`, `Busy`, stand-in `initialize`, forwarding with id rewriting,
  cancellation both ways, server requests answered, list-changed broadcast.
- SDK, real processes (python fixture): start, restart on next use after exit
  (`ServerGone`, new generation), start timeout and failure, shutdown, and the
  **stateful** fixture: a handle returned to a stand-in's call resolves in the
  gateway's later read on the same generation, and not on a new one.
- Gateway: hello refusals, a stand-in closed with its generation, the relay
  command's exit status, stand-in configuration and digest in `session/new`,
  the view's `resourceUri` and revision, the schema bound.
- Desktop: a gateway tool with a `resourceUri` maps to a `widget` part.
- Live: `scripts/mcp-test-server/live-check.mjs` with Claude and Codex.
