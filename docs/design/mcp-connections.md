# One MCP connection per server for each harness session, owned by the gateway

Design for #346, under [ADR 344](../adr/todo/344-mcp-ui.md). The ADR decides
that the gateway owns the connection to each configured MCP server and hands
the harness a stand-in. This document says how, and writes down the states
and orderings the tests come from ([gate 15](../../CODING_STANDARDS.md#gates)).

The proposed remote HTTP and OAuth extension is owned by
[ADR 392](../adr/todo/392-remote-mcp-servers.md). Its lifecycle charts, ordering
tables and fixture/implementation plan remain proposed; this guide describes the
implemented connection contract below.

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
hello and closed when the stand-in ends. Two harness sessions never share a
server process, so neither blocks, sees, or outlives the other's, and Nessa's
own `nessa` shell server runs per session as it did. Each session belongs to
the conversation its harness was opened for, by a token the gateway issued
for that open (below), so two conversations never share one, as long as the
harness passes each `mcpServers` entry's `env` to the stand-in it starts:
Claude's and Codex's do (observed live; a stand-in without the token is
refused, so one that did not would show no MCP servers at all); OpenCode's
has not been run. The SDK
(`crates/nessa-sdk/src/infrastructure/mcp/`) owns the protocol: the
handshake, `tools/list` with each tool's `_meta.ui`, `resources/read` of an
MCP App resource, forwarding a stand-in's traffic, and each session's
process. The gateway (`crates/nessa-server/src/mcp_servers/`) owns the
socket, the `mcp-relay` command, and the lookup the conversation view asks.

**Which conversation a stand-in belongs to.** What a harness is launched
with is per agent, so it can't say. Each time the SDK opens a provider
session for an SDK session — a conversation's, which is named by its id —
the binding asks the host for a grant (`StandInGrants`) and holds it for that
provider session's life, through every restart of its process.

The gateway's grant (`ConversationGrants`) is a fresh token: 32 random bytes,
of which it keeps only the SHA-256. The SDK puts it in every stand-in's ACP
`env` as `NESSA_MCP_SESSION`, the same for `session/new` and
`session/resume` and for all three profiles, and never in the arguments
([why](#mcp-servers-and-the-restoration-identity)). The relay
reads it from its environment and says it in its hello. The session it
opens is owned by that open (`McpOwner`: the SDK session and the grant).
When the provider session ends — closed, deleted, stopped, retired, shut
down, warm-up done — the grant is dropped and revoked: its token is refused
from then on, no session opens under it (one opening then is refused), and
its sessions are closed as their stand-ins ending would close them — calls
waiting end at once, the server's stdin is closed, and a server still
running two seconds later is killed — so a server that commits evidence at
end of input (Nessa's own shell server) still does.

- **Handshake.** The gateway sends `initialize` with protocol `2025-06-18`
  and `capabilities.extensions: { "io.modelcontextprotocol/ui": { mimeTypes:
  ["text/html;profile=mcp-app"] } }`, accepts a server answering `2025-06-18`,
  `2025-03-26` or `2024-11-05`, then sends `notifications/initialized`; the
  session is open then, and its tools are listed in the background. It
  declares no `roots`, `sampling` or `elicitation`.
- **What the harness is given.** In `session/new`, each configured server is
  replaced by a stand-in under the same name: the gateway's own executable,
  `mcp-relay`, the relay socket, the server's name, and a digest of the
  server's configured command, arguments and environment, keyed with a
  secret of the gateway process's own (#391). The set is read from the
  gateway's live set at each provider open, and kept for that provider
  session's life ([the live server set](#the-live-server-set-391)).
- **Stand-in traffic.** The harness's `initialize` is answered by the gateway
  from the upstream's own answer (its protocol version, capabilities, server
  information and instructions), less `resources.subscribe`;
  `notifications/initialized` is not forwarded, since the upstream was
  initialized once. `resources/subscribe` and `resources/unsubscribe` are
  refused (`-32601`): their updates are not routed back. Every other request
  is forwarded with a fresh upstream id and answered with the harness's own
  id, verbatim — except that a `tools/list` answer leaves out a tool
  that [who a tool is for](#who-a-tool-is-for) hides from the model, and a
  `tools/call` naming a tool hidden so is refused (`-32602`) — hidden by the session's own latest
  list, or by the latest list this stand-in forwarded, by the order they were
  asked in: having declared MCP Apps, the host keeps an app's own tools from
  the model. The harness's `initialize` is answered with the protocol version
  the upstream negotiated, whatever the harness asked for, as a server that
  speaks one version answers.
  `notifications/cancelled` for a forwarded request cancels it upstream. A
  server's `*/list_changed` notifications are passed on — when a stand-in
  falls too far behind to have them all, it is sent all three, which are
  idempotent; its other notifications (progress, logging) are not. A server's own requests are
  answered by the gateway: `ping` with `{}`, anything else with `-32601`,
  since the gateway declared none of them. A `tools/call` answer's
  `structuredContent` is kept for the call's tool call before the harness is
  answered ([forwarded results](#forwarded-results), #435).
- **Tool UI.** `tools/list` (paged by `nextCursor`) gives each tool's
  `_meta.ui`: an optional `resourceUri` (a `ui://` URI) and `visibility`
  (`model`, `app`), each read on its own. Who may see and call the tool is
  [who a tool is for](#who-a-tool-is-for). A tool whose `resourceUri` is
  absent or cannot be read is kept without a UI, and keeps that visibility
  (#412); one whose visibility cannot be read keeps a readable `resourceUri`
  as its UI. A `_meta.ui` that is not an object has no URI to keep. A tool
  whose name cannot be one is left out. A session's list is read when it
  opens and again on `notifications/tools/list_changed`.
- **UI resources.** `resources/read` of a `ui://` URI must answer one content
  for that URI with MIME `text/html;profile=mcp-app` and `text` (or `blob`
  holding UTF-8), with its `_meta.ui`: `csp` (`connectDomains`,
  `resourceDomains`, `frameDomains`, `baseUriDomains`), `permissions`
  (`camera`, `microphone`, `geolocation`, `clipboardWrite`, each asked for as
  `{}` or `true`; anything else, and any other permission, is never granted),
  `domain`, `prefersBorder`.
- **The view.** `ConversationTool.mcp.resourceUri` is filled when the view
  is read, from the conversation's own newest open session of the call's
  server — the one its harness talks to now — for the one listed tool the
  call names (`McpTool::names`: exactly, or in Claude's replaced spelling;
  two candidates give none). None is attached when the conversation has no
  open session of that server, or it has not listed its tools yet. A call's
  widget therefore depends on its conversation having an open session: after
  its harness session ends, or the gateway restarts, a past call shows no
  widget until its conversation is opened again and lists. The view's revision
  takes the filled URIs into account, so a window holding the same revision
  holds the same URIs. The desktop maps a gateway tool to a `widget` part with
  `toolWidget`.

## Who a tool is for

One reading of a listed tool's `_meta.ui`, for the model and for an app.
Absence of `_meta.ui`, or of `visibility` inside an object, is both: the
server did not say. A `_meta.ui` or a `visibility` that is present and cannot
be read is no one's — hidden from the model, and an app's `tools/call` is
refused `tool_not_for_app` — because it cannot be read to include either.
A `resourceUri` that cannot be read is no UI, and does not change who.

| What `tools/list` gives | Who |
| --- | --- |
| no `_meta`, or no `_meta.ui` | both |
| `_meta.ui` an object, `visibility` absent | both |
| `_meta.ui` an object, `visibility` an array of strings | who it names (`model`, `app`) |
| `_meta.ui` an object, `visibility` not an array of strings (`null` included) | no one's (#412) |
| `_meta.ui` present and not an object: a string, an array, a number, a boolean, or `null` | no one's (#424) |

## A name listed more than once

One visibility for a name that appears more than once in one `tools/list`
result, and more than once across the pages of the one read a session
keeps. A side may see it only when every entry says so (#425). An entry
whose `visibility` cannot be read, or whose `_meta.ui` is present and not
an object, is no one's (#424), so it excludes both sides.
Both orders are the same result.

When any entry excludes the model, the name is left out of the forwarded
list and a `tools/call` is refused `-32602`. When any entry excludes the
app, an app's call is refused `tool_not_for_app`. `listed_tool` returns
that one visibility, with the first entry's `resourceUri` and hints. The
view still draws no UI when more than one listed tool matches the call. A
later `tools/list` result still replaces the names it contains; it does not
reopen an entry it does not contain.

| What the entries say | Who |
| --- | --- |
| `model` on every entry | the model may see and call it |
| any entry excludes `model`, or cannot be read | hidden from the model |
| `app` on every entry | an app may call it |
| any entry excludes `app`, or cannot be read | refused `tool_not_for_app` |

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
| An MCP App's `tools/call`, `resources/read` (`call_tool`, `read_app_resource`) | the caller's budget: the gateway passes `x-mcpAppCallTiming.callTimeoutMs` and `readTimeoutMs` | `Timeout`; the connection stays |
| A forwarded request | none from the gateway | the harness's own deadline and cancellation |
| Closing a session | stdin closed, 2 s, then the process group killed | — |
| A refusal's message | 512 characters, control characters as spaces | — |
| Change notices held for a stand-in or a session's list | 16 | the stand-in is sent all three `*/list_changed`; the list is read again |
| Tool names whose visibility a session remembers | 4096 | only the names the list asked latest gave are kept (a list answered late cannot evict a later one's), then none if that list alone passes it, and a name not remembered is hidden; tools paged past it are callable only from the latest pages |
| Live grants | one per open provider session | — |
| Forwarded results a grant keeps | 32, each a call id of at most 256 bytes, the answering server's name (at most 64 bytes) and a result of at most 64 KiB (`MAX_STRUCTURED_RESULT_BYTES`) | the oldest is dropped; a larger result is kept as the "omitted: too large" text |
| Sessions a grant remembers | its open ones, and those ended since its last opening | dropped as the next opens; all taken when it is revoked |
| Open sessions | not bounded here | each is a harness's stand-in, started by a process of the gateway's own user |

## Failures are typed

`McpError`: `InvalidConfiguration(McpServerProblem)` (servers that cannot be
launched as configured, and why: too many, a name twice, a bad name,
executable or arguments, a bad, reserved or NUL-holding environment
variable), `NotConfigured` (no such server), `Start` (could not be
launched), `Handshake` (refused or unreadable `initialize`), `Timeout`,
`ServerGone` (the process ended or its pipes closed), `Remote { code,
message }` (a JSON-RPC error), `Malformed` (an answer of the wrong shape),
`TooLarge`, `NotAnApp` (not `text/html;profile=mcp-app`), `Busy`, `Stopped`
(the gateway is shutting down; also what replacing the set is refused with
then), `Closed` (the session's harness session
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
| Open | the stand-in ends: the harness closes, its socket breaks, the relay is killed, a frame that is not JSON or is past the bound | Closed | calls waiting end `Closed`, unanswered (closing the server's stdin ends them; a `notifications/cancelled` may or may not be written first); 2 s; the process group killed |
| Open | process exits, stdout closes, oversize or non-JSON frame | Gone | calls waiting end (`ServerGone`, `TooLarge`, `Malformed`); the stand-in is closed, and its relay exits; the process group killed |
| Open | gateway stops | Closed | as the stand-in ending, with `Stopped` |
| Open | the last handle to it is dropped without closing | — | the process group killed at once, even while its background list waits on the server |

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
| Hello | a line within 4 KiB and 5 s naming a live grant's token, a configured server and its current configuration | Opening | a session of its own, owned by that grant's conversation (above) |
| Hello | no token, or one never issued, or one whose grant is revoked | Closed | refused `unknown-session`, before anything is said of the server; nothing is started |
| Hello | unknown server | Closed | refused `unknown-server` |
| Hello | configuration digest differs | Closed | refused `configuration-changed` |
| Hello | oversize, unreadable, or no line in 5 s | Closed | closed without an answer |
| Opening | session open | Serving | `accepted` |
| Opening | opening fails | Closed | refused `unavailable` with the typed reason; the relay exits 1, so the harness sees its server fail to start |
| Serving | harness `initialize` | Serving | answered from the upstream's `initialize` result, less `resources.subscribe` |
| Serving | harness request | Serving | forwarded; the answer comes back with the harness's id |
| Serving | `tools/list` answer | Serving | forwarded without the tools that [who a tool is for](#who-a-tool-is-for) hides from the model ([a name listed more than once](#a-name-listed-more-than-once) when the result names one twice); of two lists answered out of order, the one asked later decides |
| Serving | `tools/call` for a tool the session's latest list, or the stand-in's, hid — or any tool before a list has said anything — | Serving | refused `-32602`, nothing forwarded ([a name listed more than once](#a-name-listed-more-than-once)) |
| Serving | the stand-in falls behind the server's change notices | Serving | sent all three `*/list_changed` notices |
| Serving | `resources/subscribe`, `resources/unsubscribe` | Serving | refused `-32601`, nothing forwarded |
| Serving | a request reusing the id of one still waiting | Serving | refused `-32600`, nothing forwarded |
| Serving | harness `notifications/cancelled` for a forwarded request | Serving | that request is dropped; upstream `notifications/cancelled` |
| Serving | harness reuses a cancelled request's id while the old call's answer is in flight | Serving | the old answer is dropped; the new request gets its own |
| Serving | harness closes, or its socket breaks | Closed | the session is closed (above) |
| Serving | session ends | Closed | the socket is closed, and the `mcp-relay` process exits at once, its stdin unread: the harness sees its server end, as when it owned the process |
| Serving | the grant is revoked (its provider session ended) | Closed | calls waiting end `Closed` at once; stdin closed, 2 s, then the process group killed, as a stand-in ending; a close already in its grace is not cut short, and every close returns only once the server is stopped |

### A grant

One per provider open of an SDK session (a conversation's, or a warm-up's).

| State | Event | Next | Effect |
| --- | --- | --- | --- |
| — | the SDK opens a provider session for `S`: new, resumed, or warm-up | Live | a token minted; its digest registered as `S`; each stand-in of the open carries it |
| Live | the harness process restarts inside that provider session | Live | the same token: the same open |
| Live | a hello names its token | Live | a session opens, owned by (`S`, this grant) |
| Live | the provider session ends: closed, deleted, stopped, retired, shut down, warm-up done, or its open fails | Revoked | the token is refused; the grant's open sessions are closed (stdin, 2 s, the process group); a session still opening under it is refused when its `initialize` is answered — the revocation and its registration share the grant's own lock — and one asked for after it launches nothing; with no runtime to close on, or one shutting down, the process groups are killed at once |
| Revoked | a hello names its token | Revoked | refused `unknown-session` |
| Live | `S` is opened again (resumed) | Live, beside the new one | the new open's stand-ins carry the new token; the view reads `S`'s newest session; this one is revoked when its provider session ends |
| — | the gateway restarts | none | every old token is refused; each conversation gets a new one when it opens |

The relay socket: an exclusive lock on `<socket>.lock`, held for the
gateway's lifetime, decides which gateway holds it. While another holds it
the bind fails and MCP servers are off for that run; once none does, a socket
an earlier run left is replaced. A gateway executable or socket
path that is not UTF-8 also leaves MCP servers off.

### Forwarded results

Claude's harness gives its model, and its ACP client, a result's
`structuredContent` only as JSON text — in `content`, `rawOutput` and the
PostToolUse frame's `toolResponse`, in place of the result's own text — so no
ACP frame says which text was structured (ADR 344's observed table). The
stand-in saw the object, and Claude's harness names the call it forwards
(`_meta["claudecode/toolUseId"]`) by the id its ACP frames give it
(`toolCallId`). Each grant (`McpOwner`) owns an
`acp::sessions::ForwardedResults`; the grant the gateway gives the binding is
the owner's own (`McpOwner::stand_in_grant`), so it cannot carry another
owner's, and the ACP worker attaches each result to its call
(`acp::sessions::forwarded::attach_forwarded`), from where it is saved and
projected as any structured result is. A call reported `completed` was
answered, and the stand-in keeps its result before giving it (S1's test holds
the stand-in inside that write), so the result is there to take. A call
reported `failed` takes nothing (W7). The pinned harness reports an MCP
`tool_result` as `status: is_error ? "failed" : "completed"` (its
`acp-agent.js`), and the recorded `always_fails` (an `isError` result) is
`failed`; so an `isError` result's structured content is not attached, its
text is. A call the harness cancels keeps nothing in the first place (S6).

| # | Event | Effect |
| --- | --- | --- |
| S1 | a `tools/call` answered with a result holding `structuredContent`, `isError` or not, its request naming a call id | its JSON text kept under that id, before the answer is written |
| S2 | as S1, past 64 KiB | the "omitted: too large" text kept, never cut JSON; at 64 KiB exactly, kept |
| S3 | a result without `structuredContent`, or `null` | nothing |
| S4 | a JSON-RPC error answer, or an answer too large for a frame (the harness gets `-32603`) | nothing |
| S5 | no call id (Codex sends none), or one the ACP binding would not accept as a tool call's id (`acp::fields::identifier`, then `ToolCallId`: a string of at most 256 bytes, not empty or blank) | nothing |
| S6 | the harness cancels the call while it waits, or after its answer is in but before the stand-in has answered it | nothing; whichever the stand-in reads first decides, so a result is kept exactly when the stand-in answers the call (an answer whose write then fails is kept, and waits to be dropped) |
| S7 | a call to a hidden tool, refused and never forwarded | nothing |
| S8 | an answer to any other method | nothing |
| S9 | 32 results kept already | the oldest dropped: usually one never taken; a burst of more than 32 results ahead of one call's completed update drops that call's, which then shows its text alone (W3) |
| S10 | an id kept again | the later result, once |
| S11 | a stand-in of one grant keeps a result | no other grant sees it, the same conversation's included |
| W1 | the call's `completed` update carrying content, naming an MCP tool, with a result kept | the result taken and appended after its content |
| W2 | an update without content (the PostToolUse frame) or before the end | nothing taken: an update without content replaces none, and one before the end has no result yet |
| W3 | nothing kept for the call: the tool returned no `structuredContent` | the update as it was: the text alone |
| W4 | a second `completed` update of the call | nothing more: taken. Carrying content, it would replace the call's content in the view, the result with it; the pinned harness sends one `completed` update per call |
| W5 | an update naming no MCP tool | nothing taken |
| W6 | an open without a grant | the update as it was |
| W7 | the call's `failed` update | nothing taken; the result waits to be dropped |
| W8 | a `completed` update whose MCP tool names another server than the one that answered under its id | nothing taken: the result is kept with its server, and is not that call's |

A result kept and never taken — a call its harness abandons or reports
`failed` — waits until it is dropped (S9) or its grant is revoked. A process
of the gateway's own user holding a live token (above) could keep a result
under a call id it guesses — but only as the server its stand-in serves,
so it attaches only to a call to that same server (W8). Keeping it replaces
whatever was kept under that id (S10), whichever server kept it, so a guess
can also make the real call show its text alone; such a process can already
push every result out (S9), so it can lose an attachment, never forge one
onto another server's call. An MCP App's own `tools/call` does not pass
through a stand-in and keeps nothing. Appending a result counts toward the
execution's retained tool bytes like any other content, as Codex's
structured results already do.

## What one connection per harness session means

- **Restoration identity.** The server list is not part of it; what follows
  is in [MCP servers and the restoration identity](#mcp-servers-and-the-restoration-identity).
- **Restarts.** A gateway restart ends every session with the agents. A
  restored conversation's harness opens new sessions through its stand-ins; a
  handle an old session gave out is unknown to the new one, and the server says
  so — as it did when the harness owned the process. Its past calls show no
  widget until a session of their server has listed its tools.
- **A server that exits** ends its session and its stand-in, and the relay
  exits, so its harness sees its server end, as when it owned the process.
  Other conversations' sessions of the same server are untouched.
- **What a server is started with.** The workspace as its working directory,
  and `HOME`, `USER`, `LOGNAME`, `TMPDIR`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`
  and the agents' search path from the gateway's own environment; its standard
  error goes to the gateway's. It used to be whatever its harness gave it.
- **What the token keeps apart, and what it does not.** It keeps one
  conversation's stand-ins from being taken for another's, and a stand-in of
  an ended open from reaching a new one. It is not a secret from processes of
  the gateway's own user: a stand-in's environment can be read by them (as
  `ps -E` shows on macOS), and so can the socket be opened. An agent's own
  shell, running as that user, could therefore present another live
  conversation's token. Part (b) of #348 has to hold that in its policy: an
  app method acts on the conversation the authenticated caller names, never
  on whatever a stand-in claims.
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
  UI, unreadable `_meta.ui`, a `_meta.ui` that is not an object
  (`a_meta_ui_that_is_not_an_object_is_no_ones`), a name listed twice
  (`a_name_listed_twice_is_judged_the_same_for_the_model_and_the_app`,
  `a_name_split_across_the_pages_of_one_read_is_judged_once`), list bounds and order, read of an app resource,
  wrong MIME, oversize HTML and frame, `Timeout` and the late answer,
  `Remote`, `Busy`, the stand-in's `initialize`, forwarding with id rewriting,
  cancellation both ways, id reuse, hidden tools, subscriptions, server
  requests answered, list-changed notices, sessions apart from each other,
  the view's lookup of a conversation's own newest session and not yet
  listed, the lookup never deadlocking with sessions ending, a revoked grant
  closing its connections before `revoke` returns and its sessions and no
  others, a grant revoked before its session opens launching nothing, one
  revoked off any runtime killing at once, a grant revoked
  while its session opens refusing that opening, a grant revoked while its
  session closes leaving every close waiting for the stop, stop racing an
  opening; forwarded results kept by the stand-in, S1–S8 and S11 (`forwarded.rs`); a provider open holding its session's grant until it ends and
  through a relaunch of its process, the open naming the manager's session, every
  `mcpServers` entry carrying the open's environment, and grants and the MCP
  server list left out of the fingerprint, with a restore resuming under the
  current list (`adding_`, `editing_`, `moving_an_mcp_servers_command_` and
  `removing_an_mcp_server_keeps_the_identity_and_restores`).
- SDK, real processes (python fixture): launching as configured, separate
  processes per session, a long call in one not blocking another, killing one
  leaving another, closing stopping the process group, the session ending
  with its stand-in, and the **stateful** fixture: a handle returned to a
  stand-in's call resolves in a later read on the same session, and not on
  another.
- Gateway: the composed agents' grants being the ones the composed relay
  lets through, the launch configuration carrying them, the view keyed by its
  conversation's session, hello refusals (no, forged, and revoked tokens among
  them, a forged one starting nothing), a
  token mapping to its conversation while its grant lives, an open's grant
  holding the forwarded results of the owner its token resolves to and no
  other open's, a revoked grant
  ending the sessions it opened, two conversations on one server and a
  resumed one each on sessions of their own, no token when the random source
  fails (its stand-ins refused), a hello's `Debug`
  never printing its token, the relay command's exit and its token read from
  its environment, the stand-ins and digest in `session/new`, a relay process killed outright ending its server's
  process group, a relay exiting when its server ends with its stdin still
  open, the view's `resourceUri` and revision, the schema bound.
- SDK, ACP: the store's S9 and S10, and W1–W8
  (`tests/infrastructure/acp/sessions/forwarded.rs`), and
  Claude's recorded frames (`report_rows` completed, `always_fails` failed)
  replayed through the worker with a grant holding a result for each
  (`contracts/tools.rs`).
- Desktop: a gateway tool with a `resourceUri` maps to a `widget` part.
- Live: `scripts/mcp-test-server/live-check.mjs` with Claude and Codex.

## The live server set (#391)

`McpServers` is the one owner of the configured set: it holds the launches
behind one swap, `replace(launches)` validated and refused once stopping,
and `configured()`. Nothing else keeps a copy. A provider open reads the
stand-ins for the set as it is then (`AcpConfig.mcp_servers`, an
`McpServerList` read from the gateway's `StandIns`, where the open's grant is
read too) and keeps them for its provider session's life, through every
restart of its process; the relay reads the set's digests at every hello.
On Unix the relay exists even with no server configured, so a set replaced
later reaches the next open without a restart. A socket that cannot be bound
still leaves MCP servers off for the run.

The rules for a set have one owner, `StdioMcpServer::problem_in` (at most
`MAX_MCP_SERVERS` = 16, each server's `problem`, names of their own), with
`McpServerLaunch::problem` adding what a process may be given:
environment names of ASCII letters, digits and `_`, 1–256 bytes, not starting
with a digit; never `NESSA_MCP_SESSION` (`MCP_SESSION_VARIABLE`, the
stand-ins' token); no NUL in a value. `AcpConfig::validate`, `McpServers::new`
and `replace` all ask it.

A stand-in is opened only on the configuration it was admitted against:
the relay admits a hello against the digests, then opens what it admitted
with `McpServers::open_as`, which refuses `ConfigurationChanged` (or
`NotConfigured`) when the set was replaced in between. So a stand-in is never
served by a different configuration under the same name, which is what makes
what [MCP servers and the restoration identity](#mcp-servers-and-the-restoration-identity)
relies on.

The digest covers what a server is started with: its command, its
arguments, and its whole environment, names and values
(`infrastructure::launch_digest` chooses the fields, once, for the stand-ins
and the relay alike; `open_as` compares the whole launch). A server whose
variables alone changed is a changed server, so its old stand-in is refused
`configuration-changed` as an edited command's is. The digest is HMAC-SHA256
under a key drawn for each gateway process (`ConfigurationKey`) and held only
in its memory: a stand-in's arguments, which a process list shows, carry
neither a value nor anything a guessed value could be checked against. Its
digest changes from run to run, which nothing minds: a restart ends every
stand-in, and the restoration identity reads none of it
([MCP servers and the restoration identity](#mcp-servers-and-the-restoration-identity)).

### Managing the stored servers

`config.json`'s `agents.mcpServers` holds each server as
`{name, command, args?, enabled?, env?}`; one without `enabled` is on, one
without `env` has no variables of its own (the current contract's default).
`mcp_servers::infrastructure::stored_servers` is the one reader and writer of
that shape, at startup (`AgentsConfig`) and on each change. In the gateway
it is a `ConfiguredMcpServer { server, enabled, env }`; the SDK's
`StdioMcpServer` does not grow. A server's environment is the gateway's base
(`server_environment`) with its own `env` over it — its value wins — built in
one place, `LaunchSettings`, at startup and after each change. A server
turned off stays in the file and out of the live set; it is still checked,
and still counts towards `MAX_MCP_SERVERS`.

A server's variables are named once. `ConfiguredMcpServer::new` is the one
rule — its fields are private, so nothing builds one around it — and
`stored_servers` reads `env` entry by entry into it, so a name given twice
in the file is refused, in either order, before decoding into a map could
keep one value silently: at startup by the runtime configuration's parse,
and on a write by the store's check of the file it reads
(`mcp_servers_config_invalid`,
`a_repeated_variable_name_in_the_file_is_refused_in_either_order`). The
SDK's numeric bounds — `MAX_MCP_SERVERS`, a name's bytes, the arguments'
count and bytes, a variable name's bytes — are published in the product
schema as `x-mcpServerRules`; the generator checks each value against the
SDK constant it names and refuses to publish when they differ, and the
client reads them as `mcpServerRules`.

`nessa` is Nessa's own server (`MANAGED_SERVER_NAME`). The desktop drops any
stored `nessa` and adds the bundled one in memory; the gateway takes it from
the startup configuration into `LaunchSettings`, lists it `managed`, and
refuses every save or remove naming it (`mcp_servers_reserved_name`). A write
edits the file, not the composed configuration, so the file never gains it.

The desktop drops a stored `nessa` from its startup copy only; the file can
still hold one, perhaps with a stale executable and variables that are
secrets. On the desktop it is never used: never launched, inspected or
listed, since the bundled one stands in its place. So the next applied
change, a save or a remove of any other server, drops it from the file. The
outcome's `before` names it and its `after` does not
(`a_desktop_change_drops_a_stored_nessa_and_its_audit_says_so`). A refused or
failed change writes nothing, and leaves it there. Composition says which
gateway this is: the desktop's `bundle` (`packaged_agents`) reaches
`LaunchSettings`, and the settings read it as `LiveServerSet::bundled`.

On a gateway without the desktop, a `nessa` stored at startup is the managed
server, whether it is on or off: one rule, the stored-server rule, applied to
it. It is listed once, `managed`, with its own `enabled` and variable names;
it counts towards `MAX_MCP_SERVERS` and is checked with the rest of the set
either way; it is launched only when on. It is read once, at startup — as the
desktop's is composed once — so a hand edit to it in the file reaches the
next start, and an entry under that name is never launched in its place
(`a_stored_nessa_on_a_headless_gateway_is_the_managed_server_on_or_off`).

`mcpServers.list`, `mcpServers.save` and `mcpServers.remove` all ask for
`credential.manage`; a caller without it is refused `forbidden` before the
params are read.

- `mcpServers.list` → `{revision, servers: [{kind: "stdio", name, command,
  args, envNames, enabled, managed}]}`: stored order, then the managed server;
  `envNames` sorted by name, as the variables are stored. It reads the stored
  file, not the live set: a hand edit to `config.json` is listed at once, and
  reaches the live set — and new conversations — at the next save or remove,
  or the next start. `enabled` is whether the server is stored on, so the
  same holds for it.
- `mcpServers.save {revision, previousName?, server: {kind: "stdio", name,
  command, args, env: [{name, value | null}], enabled}}` → `{revision,
  live}`. The server's `env` is exactly the names listed; `value: null`
  keeps the value stored for that name on the server being saved (the one
  under `previousName`, else under `name`) only when the save launches that
  server exactly as stored apart from the kept values: the same command and
  arguments, and the same variable names, each other one `null` or given its
  stored value ([LS17](#the-live-server-set-391)). A variable such as
  `LD_PRELOAD`, `NODE_OPTIONS` or `PYTHONPATH` loads code as surely as a new
  command does, so adding one, changing one, or leaving one out means giving
  every value again. A save whose resulting list would not fit one frame is
  refused `mcp_servers_config_too_large`, even one that shortens a list
  already past it: that is intended, and remove is the way out of such a
  list.
- `mcpServers.remove {revision, name}` → `{revision, live}`. A remove is
  never refused for a bound on the list (count, frame size, an entry that
  breaks a rule): it only shortens the list and is how a hand-edited one is
  brought back. Its file is written, and it always takes its server out of
  the live set: when the list it leaves cannot go live as a whole, the live
  set becomes the current one less that server — a subset of a valid set,
  so always valid. The outcome says `liveSet: withdrawn` when that server
  was live, and `liveSet: kept` when it was not (a server turned off, or
  one only the hand-edited file named), since the live set did not change
  (`a_remove_takes_its_server_out_of_the_live_set_while_the_list_is_past_a_bound`,
  `a_remove_from_a_list_past_its_bounds_is_written_and_recovers`).
- `live` (save and remove) is whether new conversations now get the stored
  list as written: `false` while the gateway stops, or after a remove that
  could not make the list live, whether it withdrew its server or left the
  live set as it was (`a_remove_answers_whether_the_list_went_live`).

The revision is a digest of the stored block (`[]` when there is none),
keyed with the process's `ConfigurationKey` — the one the stand-ins' digests
are keyed with (`stored_revision`) — so it gives nothing to test a guessed
variable value against
(`the_revision_is_keyed_and_changes_with_a_variables_value`). It changes
across a restart — the key is minted per process — which costs a caller
holding one from before it one conflict. Nothing beside it is persisted.

Each admitted `save`, `remove` and `inspect` has one owner: a task
`McpServerSettings` spawns and tracks, which runs from the `requested` record
to the outcome record — publish, replace the live set, record — whether or
not the caller's future is still polled
(`a_caller_gone_mid_write_still_replaces_the_live_set_and_records_the_outcome`).
The audit evidence never depends on the response. An admitted change that
runs to completion leaves the file and the live set agreeing — one published
while the gateway stops excepted (LS14), which the next start reads; a
change made to the file by hand reaches the live set at the next change. A
task that panics answers by how far it got, which it marks as it goes: past
the publish — for an inspection, once its server's launch has begun (the
inspector marks it just before it asks the SDK to start the server) —
`audit_unavailable` with `applied: true`, as its outcome was never recorded;
before it, `mcp_servers_storage_unavailable` with `applied: false`, and the
owner, not the caller, writes a `failed` outcome with reason `panicked` once
the `requested` record was written.

Gateway shutdown closes admission as it begins
(`ProductRouteState::close_mcp_server_admission`, beside watch admission,
before the conversations drain): a later request answers
`mcp_servers_stopping`, unaudited, having started nothing. The same close
stops the inspections under way at once — they change nothing, so they are
not waited out: one not yet started is recorded `stopping` with
`started: false`, one started is cut (`cut: stopping`, no tools) and its
process group killed
(`x_early_admission_closes_and_inspections_are_cut_while_conversations_drain`).
The MCP stop has one owner, `composition::mcp_servers::stop`, which the
gateway's cleanup (`cleanup_product`) runs after conversations. It closes
again, which changes nothing, then waits for every admitted task to record
its outcome (`McpServerSettings::shutdown`), then stops the servers
(`McpServers::stop`). The wait is bounded by the store's lock wait (2 s)
plus 5 s for the file system (`drain_bound`), well inside the 30 s the
supervisors give a stopping gateway before they kill it (launchd's
`ExitTimeOut`, systemd's `TimeoutStopSec`). A drain that runs out of time
still stops the servers, but is a shutdown failure: the report's MCP stop
is `Failed(Unfinished { running })` and does not confirm
(`shutdown_during_a_save_returns_after_its_outcome_is_recorded`,
`shutdown_stops_a_running_inspection_and_records_it_cut`,
`a_request_after_shutdown_began_is_stopping_and_starts_nothing`,
`the_mcp_stop_drains_admitted_writes_before_the_servers_stop`,
`an_unfinished_drain_is_an_unconfirmed_shutdown`).

Each change, in order:

1. The `requested` record (`…/conversations/audit/mcp-servers`), with the
   target, the caller's revision, the initiator and, for a save, the server
   asked for — `{name, command, args, enabled, envNames}` — so a refused save
   still names the executable it asked for
   (`a_refused_save_still_records_the_executable_it_asked_for`). When
   it cannot be written: `audit_unavailable` (`applied: false`), and nothing
   is locked, written or applied.
2. `config.json.lock`, tried every 20 ms for at most 2 s on the gateway
   clock; still held: `mcp_servers_busy`. The lock file is opened without
   blocking and must be a regular file, so one planted as a FIFO is refused
   (`mcp_servers_storage_unavailable`) rather than holding a blocking thread
   in `open`
   (`a_lock_that_is_not_a_regular_file_is_refused_without_blocking`).
   The dev script (`scripts/dev-agent-config.mjs`) takes this same lock
   when it writes `agents`: create the file (0600, no follow, no block),
   require a regular file, and take an exclusive non-blocking `flock` on
   that descriptor. Node has no `flock`, so Perl calls it on the descriptor
   the script already opened. The file is not deleted. A file left behind,
   empty or still carrying an older pid line, is not a holder — the flock
   is — and deleting it would split the lock onto a new inode. The script
   stands down when the flock is held and when that helper cannot run, so
   it does not write beside a lock it does not hold. Closing the descriptor
   releases the flock; a killed run releases it the same way, when the
   process closes the descriptor
   (`an empty lock file left by the gateway is not a holder`,
   `a flock held on the lock file makes the script stand down`,
   `a lock that is not a regular file is refused without blocking`,
   `a symlink lock is refused`).
3. Read the file, checked by the runtime configuration's own parse and its
   64 KiB bound (`RuntimeConfig::parse`, `MAX_CONFIG_BYTES`); compare the
   revision; make the edit; check the result with the SDK's rules
   (`McpServerLaunch::problem_in`, managed server included).
4. Re-read the file and compare its revision with the one the edit was made
   to — an edit made outside the lock since the read is
   `mcp_servers_revision_conflict` with the revision now
   (`a_change_made_outside_the_lock_after_the_read_is_a_conflict`). That
   narrows the window for an edit outside the lock; it does not close it: one
   that lands between this re-read and the publish is overwritten. Write the
   whole file with only the block replaced, check it again, and publish it in
   one step, private (0600). The lock travels into each blocking read and
   write and back out, so nothing is written outside it. The whole file is
   re-serialised: the gateway owns `config.json`, so its key order and layout
   after a write are the gateway's — pretty-printed, or compact when only
   that fits the 64 KiB bound, which is on the bytes written, so a remove can
   always shrink a file read within it — and everything else in it keeps its
   value, not its spelling. A file with no `agents` block — or with
   `"agents": null`, which the runtime configuration reads as none — gains
   one from the running catalog and workspace
   (`c_null_agents_is_read_and_written_as_absent`); the desktop makes its
   default workspace whenever that is the one configured, so the next start
   does the same with the block as without it. Once the file is renamed into
   place the change is published, whatever the directory sync then says: a
   sync that fails leaves the change applied, not durable
   (`s_sync_a_publish_whose_directory_sync_fails_is_applied_not_durable`).
5. Replace the live set, still under the lock, so changes publish and
   replace in the same order: a second writer does not take the lock until
   the first's replacement is done
   (`a_second_writer_waits_for_the_first_writers_live_replace`). The live
   set is not replaced for one of two reasons. The gateway is stopping: the
   set is kept, the outcome says `liveSet: kept`, and the next start reads
   the file (`a_publish_during_stop_answers_success_and_leaves_the_live_set`).
   Or a remove left a hand-edited list still past a bound: its server is
   taken out of the set and the rest kept, the outcome says
   `liveSet: withdrawn`, and the change that brings the list within its
   bounds takes it live
   (`a_remove_takes_its_server_out_of_the_live_set_while_the_list_is_past_a_bound`).
   Either way the change answers success, with `live: false`.
6. Unlock, then the outcome record: `applied` with the revision and names
   before and after, what it did to the live set (`liveSet`: `replaced`,
   `withdrawn` or `kept`) and whether it was made `durable`, or
   `refused`/`failed` with the reason and what was stored when it was read.
   Applied but not durable answers `mcp_servers_storage_unavailable` with
   `applied: true`. Each side names the target as stored there —
   `{name, command, args, enabled, envNames}`, before under `previousName`
   for a rename, `null` where none is stored
   (`the_audit_records_the_targets_before_and_after_on_save_rename_disable_and_remove`). When it cannot be written: `audit_unavailable`
   with whether the change applied and, when it did not, the `code` it
   would have been answered with, so neither cause is lost. Nothing is
   rolled back.

A record names servers and variables, never a variable's value; so do the
wire and every `Debug` (`ConfiguredMcpServer`, `ServerSave`,
`McpServerLaunch`). Its `cause` is the operation's cause:
`caller_requested`, with the caller as `initiator`, for everything the
caller's operation ran into — a deadline, a refusal or a failure among them;
`gateway_stopping`, with `initiator: {kind: "system"}`, for an inspection's
outcome when shutdown ended it — not started, or cut — its caller still on
the `requested` record with the same `operationId`
(`shutdown_ended_inspections_are_recorded_as_the_gateway_stopping`). Records
carry no sequence number: an operation's `requested` record is made durable
before its outcome, and between operations — and across a restart — the
only order is `observedAtMs`, on the wall clock, which may step backwards. The file is never repaired: one that does not parse is
refused `mcp_servers_config_invalid` before and after the edit. So is one
holding a stored name longer than `MAX_MCP_SERVER_NAME_BYTES` (LS20): every
other rule for a stored server is the SDK's, asked of the whole list and
named in `mcp_servers_invalid`, but a request naming a longer one would not
fit a frame, so it could not be removed through the gateway. The refusal
carries no details, so the gateway logs which entry it was, by its index and
its name's length, never the name or a value.

Errors are `McpServersErrorCode`: `mcp_servers_not_configured`,
`mcp_servers_invalid` (details `{problem, server?, name?}`: `server` names the
server for every problem but `too_many` — an entry added to the file by hand
too, as startup's error does — and `name` the variable, `environment_name`'s
among them; a repeated name is said before a missing value),
`mcp_servers_reserved_name`,
`mcp_servers_not_found`, `mcp_servers_revision_conflict` (details
`{revision}`), `mcp_servers_busy`, `mcp_servers_config_invalid`,
`mcp_servers_config_too_large`, `mcp_servers_storage_unavailable` (details
`{applied}`: `true` only for a change published whose directory sync
failed),
`audit_unavailable` (details `{applied, code?}`), `mcp_servers_stopping`, and
the inspection codes below. `mcp_servers_not_configured` means
this gateway holds no live set to manage: not Unix, no agents configured, or
MCP off this run because the relay socket could not be bound, a path is not
UTF-8 — the running catalog or workspace, which a first write to a file with
no `agents` block would store, among them (LS21) — or no key for the
configuration digests could be drawn. A gateway that holds one always manages it, with or without
servers configured.

An `env` entry must carry `value`: `null` is the explicit keep, and an entry
without `value` is refused `invalid_request` rather than read as one. The
generator makes every required nullable field required on the wire this way.

Row labels are the PR 2 design's (on #391), prefixed `LS` so they do not
meet the stand-in and forwarded-result rows above.

| # | State / event | Expected | Test |
| --- | --- | --- | --- |
| LS1 | Caller lacks `credential.manage` | `forbidden` before params are read; nothing audited, locked or written | `s1_mcp_servers_are_forbidden_without_credential_manage_before_params` |
| LS2 | `save` or `remove` with a stale revision | `revision_conflict` with the current revision; nothing written; `requested`, then `refused` | `s2_a_stale_revision_is_refused_with_the_current_one_and_nothing_is_written`, `mcp_servers_on_the_wire_carry_names_only_and_typed_refusals` |
| LS3 | Two saves at one revision at once | The lock serialises them: with the first held between its re-read and its publish, or in its live set replacement, the second does not lock or read; the first wins, the second gets LS2 | `s3_two_saves_at_one_revision_are_serialised_and_the_second_conflicts`, `s3_two_saves_at_one_revision_over_the_real_lock_are_serialised`, `a_second_writer_waits_for_the_first_writers_live_replace` |
| LS3a | The file edited outside the lock between a change's read and its re-read | `revision_conflict` with the revision now; nothing written; the live set kept. Between the re-read and the publish it is overwritten: narrowed, not closed | `a_change_made_outside_the_lock_after_the_read_is_a_conflict` |
| LS3b | The caller goes away while the write is under way | The change's own task runs on: the lock held until the write has finished, the live set replaced, the outcome recorded | `a_caller_gone_mid_write_still_replaces_the_live_set_and_records_the_outcome` |
| LS3c | Shutdown while a save or remove runs | Admission closes; the MCP stop returns only after its outcome is recorded — its live set replaced — and only then stops the servers, within `drain_bound` | `shutdown_during_a_save_returns_after_its_outcome_is_recorded`, `the_mcp_stop_drains_admitted_writes_before_the_servers_stop` |
| LS3e | Shutdown while an inspection runs, started | Stopped at once, not waited out: outcome `inspected`, `cut: stopping`, no tools, recorded before the MCP stop returns; its process group killed; the stop returns well within `drain_bound` | `shutdown_stops_a_running_inspection_and_records_it_cut`, `shutdown_cuts_an_inspection_blocked_mid_read_and_records_it_before_the_stop`, `a_stop_mid_read_cuts_the_inspection_and_kills_its_group` |
| LS3f | Shutdown while an inspection is admitted, not yet started | Nothing launched: `mcp_servers_stopping`, recorded `failed`, `stopping`, `started: false` | `a_stop_given_before_the_launch_starts_nothing`, `an_inspection_is_not_started_unaudited_and_keeps_both_causes`, `a_stopping_record_says_the_server_was_not_started` |
| LS3g | The drain runs out of time (a write held past `drain_bound`) | The servers stop all the same; the shutdown report's MCP stop is `Failed(Unfinished { running })`, unconfirmed | `an_unfinished_drain_is_an_unconfirmed_shutdown` |
| LS3h | `drain_bound` against the supervisors' 30 s stop window | The lock wait plus the grace, not an inspection's deadline: at most a quarter of the window | `the_drain_bound_is_inside_the_supervisors_stop_window` |
| LS3i | An owned task panics after the publish, or once the inspected server's launch began | `audit_unavailable`, `applied: true`, the file new, no outcome record. The lock and the admission let go | `a_panic_after_the_publish_answers_applied_and_before_it_storage_unavailable` |
| S-panic-pre | An owned task panics before the publish, or before the inspected server's launch began | Outcome `failed`, reason `panicked` (an inspection's `started: false`), written by the owner; wire `mcp_servers_storage_unavailable {applied: false}`; nothing written. That outcome unwritable too: `audit_unavailable {applied: false, code: mcp_servers_storage_unavailable}` | `a_panic_after_the_publish_answers_applied_and_before_it_storage_unavailable`, `a_panic_whose_outcome_cannot_be_recorded_keeps_both_causes` |
| X-early | Shutdown begins, conversations still draining | Admission closed, so new requests get `mcp_servers_stopping`; running inspections cut now, recorded `gateway_stopping`; the MCP stop, and the servers' stop, only after the conversations | `x_early_admission_closes_and_inspections_are_cut_while_conversations_drain` |
| LS3d | A save, remove or inspection once shutdown has begun | `mcp_servers_stopping`; nothing locked, read, started or recorded | `a_request_after_shutdown_began_is_stopping_and_starts_nothing`, `mcp_servers_on_the_wire_carry_names_only_and_typed_refusals` |
| LS4 | Lock held past its bound | `busy`; nothing written | `s4_a_lock_held_past_its_bound_is_busy_and_nothing_is_written`, `composed_settings_publish_privately_under_the_lock_and_audit_without_values` |
| LS5 | Publish fails | `storage_unavailable {applied: false}`; the old file and live set kept; outcome `failed` | `s5_a_failed_publish_keeps_the_old_file_and_live_set`, `mcp_servers_on_the_wire_carry_names_only_and_typed_refusals` |
| S-sync | Rename ok, directory sync fails | The live set follows as far as it can (replaced, withdrawn, or kept), as after any publish; a client lists again to see it. Outcome `applied`, `durable: false`. Wire `storage_unavailable {applied: true}`; that outcome unwritable: `audit_unavailable {applied: true, code: mcp_servers_storage_unavailable}` | `s_sync_a_publish_whose_directory_sync_fails_is_applied_not_durable`, `mcp_servers_on_the_wire_carry_names_only_and_typed_refusals` |
| LS6 | `requested` can't be written | `audit_unavailable`; no lock, no write, no apply | `s6_an_unwritable_requested_record_stops_everything` |
| LS7 | Published, then the outcome fails | `audit_unavailable` with `applied: true`; the file and live set are new. Refused or failed, then the outcome fails: `applied: false` with the refusal's `code` | `s7_an_unwritable_outcome_after_a_publish_says_it_applied`, `mcp_servers_on_the_wire_carry_names_only_and_typed_refusals` |
| LS8 | `config.json` doesn't parse, before or after the edit | `config_invalid`; nothing written, nothing repaired | `s8_a_configuration_that_does_not_parse_is_refused_and_never_repaired` |
| C-dup | `config.json` env with a repeated name, in either order | `config_invalid` at startup and on write; never repaired | `a_repeated_variable_name_in_the_file_is_refused_in_either_order`, `a_configured_server_refuses_a_repeated_variable_name` |
| C-null | `"agents": null` | Treated as absent by both the reader (no agents) and the writer (no stored server; a save writes a block from the running catalog and workspace) | `c_null_agents_is_read_and_written_as_absent`, `c_null_agents_is_no_block_to_the_store` |
| LS9 | The result would pass 64 KiB | `config_too_large`; nothing written. Exactly 65536 bytes is read and written; one past it pretty-printed but within it compact is written compact; 65537 compact is refused; at the edge a remove still writes | `s9_a_result_past_the_bound_is_refused_and_nothing_is_written`, `the_configuration_bound_holds_at_exactly_its_edge` |
| LS10 | A 17th server, a bad name, a duplicate, `nessa`, a bad or reserved variable name | `invalid` with the typed problem and the server it is about, or `reserved_name`; nothing written | `s10_an_invalid_or_reserved_server_is_refused_with_its_problem`, `no_edit_names_the_managed_server` |
| LS10a | An entry added to the file by hand breaks a rule | Named: startup's error, and `mcp_servers_invalid`'s `server`, say which | `every_problem_about_one_server_names_it`, `servers_that_cannot_be_launched_as_configured_are_an_agent_error_naming_the_server`, `mcp_servers_on_the_wire_carry_names_only_and_typed_refusals` |
| LS11 | A server edited while a conversation's harness has it — its command, arguments, or only its variables | The running harness and its server process are untouched (a relaunch of the same provider session keeps its set); its stand-in's next hello is refused `configuration-changed`; the next open gets the new stand-in | `a_replaced_set_refuses_old_stand_ins_and_leaves_running_ones_alone`, `s11_to_s13_a_replaced_set_reaches_the_next_open_and_old_stand_ins_are_refused`, `each_open_reads_the_hosts_servers_and_keeps_them_through_a_relaunch`, `a_replaced_set_is_read_by_the_next_opening_and_leaves_open_sessions_alone` |
| LS12 | A server removed or turned off while open | Its stand-in's next hello is refused `unknown-server`; a new open does not list it; an open session of it is untouched | the same, `a_disabled_server_stays_stored_and_out_of_the_live_set` |
| LS13 | Added back, or turned on again | It is in the next open, and its stand-ins are let through | the same |
| LS14 | `replace` once stopping, or contending with the real `stop` for the lock | Once stopping: refused `Stopped`, the set kept, nothing launched; contending, its answer agrees with the set whichever takes the lock first. A `save` or `remove` that publishes then answers success with `live: false`, outcome `liveSet: kept` | `a_replacement_once_stopping_is_refused_and_launches_nothing`, `a_replacement_contending_with_the_real_stop_agrees_with_the_set`, `a_publish_during_stop_answers_success_and_leaves_the_live_set` |
| LS15 | Rename (`previousName`) | One write: the old name gone, the new one in its place; unknown `previousName` → `not_found` | `s15_a_rename_is_one_write_and_an_unknown_previous_name_is_not_found` |
| LS16 | Remove an unknown name | `not_found`; nothing written | `s16_removing_an_unknown_name_is_not_found` |
| LS17 | `save` keeps a variable with `value: null` | The stored value is kept only when the save launches the server as stored apart from the kept values: the same command and arguments, the same variable names, each other one `null` or its stored value. A changed command or argument, a variable added (`LD_PRELOAD`), swapped, left out, or given another value → `invalid` (`environment_value_missing`), so a secret never reaches code it was not given to; a null for a name with no stored value → the same; an entry with no `value` at all → `invalid_request`; a name given twice → `environment_name_repeated`, said before a missing value | `s17_a_null_value_keeps_the_stored_one_and_needs_one_to_keep`, `a_kept_value_is_refused_when_anything_else_in_the_launch_changes`, `mcp_servers_on_the_wire_carry_names_only_and_typed_refusals`, `a_repeated_name_is_said_before_a_missing_value` |
| LS19 | `remove` of a live server from a hand-edited list still past a bound after it | Written; the server leaves the live set and the rest stay; outcome `liveSet: withdrawn`; wire `live: false`. The remove that brings the list within its bounds answers `live: true`, outcome `replaced` | `a_remove_takes_its_server_out_of_the_live_set_while_the_list_is_past_a_bound`, `a_remove_from_a_list_past_its_bounds_is_written_and_recovers`, `a_remove_answers_whether_the_list_went_live` |
| LS19a | `remove` of a server not live — turned off, or added by hand and never live — from a list still past a bound after it | Written; the live set as it was; outcome `liveSet: kept`, never `withdrawn` | `a_remove_of_a_server_not_live_from_a_list_past_a_bound_keeps_the_set` |
| LS20 | A stored name longer than `MAX_MCP_SERVER_NAME_BYTES` (64), by one byte or by most of the file | The configuration does not parse (`stored_servers`, the one reader for startup and the store): list, save, remove and inspect answer `mcp_servers_config_invalid`, nothing written, so every request naming a stored server fits a frame. The `auth` CLI commands, which read the file through the same `RuntimeConfig::load`, refuse it too. The entry is logged by its index and its name's length, never the name or a value. 64 bytes is read | `a_stored_name_past_the_sdks_bound_makes_the_configuration_invalid`, `a_name_past_the_bound_is_logged_by_its_index_and_length_alone` |
| LS21 | The running catalog or workspace path not UTF-8, which a first write to a file with no `agents` block would store | No settings composed, logged: every method answers `mcp_servers_not_configured`, with or without an `agents` block in the file; nothing written, never a lossy path the next start would use | `a_fallback_path_that_is_not_utf8_composes_no_settings` |
| LS18 | No servers at startup, then one added | The relay exists; a new open gets the server and its stand-in is let through | `s18_with_no_server_configured_the_relay_exists_and_a_server_added_reaches_the_next_open` |
| — | A replacement that breaks a rule | Refused `InvalidConfiguration` with the problem; the set is kept | `an_invalid_replacement_is_refused_and_keeps_the_set`, `the_sets_count_and_each_servers_environment_are_checked_by_one_owner` |
| — | `replace` lands between a hello's admission and its open, before the open reads the set (after it, LS12) | The open is refused as the admission would refuse it now: `configuration-changed` for an edit, `unknown-server` for a removal; nothing launched | `a_replacement_between_admission_and_opening_refuses_the_opening`, `an_opening_admitted_on_a_replaced_configuration_is_refused` |
| — | A variable's value | Never in the wire, `list`, the audit, or a `Debug`; nor in a stand-in's arguments or the revision, raw or as an unkeyed hash | `a_launch_prints_its_environment_names_never_its_values`, `a_configured_server_prints_its_variable_names_never_their_values`, `launch_settings_print_names_never_values`, `the_list_names_each_variable_and_marks_the_managed_server`, `a_stand_ins_arguments_reveal_nothing_about_a_variables_value`, `the_revision_is_keyed_and_changes_with_a_variables_value` |
| — | A stored `nessa` on a gateway without the desktop, on or off | The managed server: listed once with its `enabled` and variable names, counted, launched only when on | `a_stored_nessa_on_a_headless_gateway_is_the_managed_server_on_or_off` |
| — | A stored `nessa` on a gateway without the desktop, then a save or remove of another server | Kept in the file with its variables; the outcome names it before and after | `a_headless_change_keeps_the_stored_nessa` |
| — | A stored `nessa` on the desktop, then a save or remove of another server | Never listed; dropped from the file, its variables with it; the outcome names it before, not after | `a_desktop_change_drops_a_stored_nessa_and_its_audit_says_so` |
| — | No stored `nessa` on the desktop, then a save or remove | The file holds exactly the edited list | `a_desktop_change_with_no_stored_nessa_drops_nothing_else` |
| — | A server saved while an agent with tools off is composed | That agent is given no MCP servers or grants; its opens and deletes go ahead | `a_tools_disabled_agent_opens_and_deletes_with_a_server_saved` |
| — | A server name starting or ending with `_` | `invalid` (`name`): `mcp__<server>__<tool>` would not say where the server ends | `a_server_name_may_not_start_or_end_with_an_underscore` |
| — | The digest | Changes with the command, each argument, each variable's name and value, and the key | `the_digest_changes_with_the_command_each_argument_each_variable_and_their_boundaries`, `each_server_is_handed_over_as_a_relay_under_its_own_name` |
| — | A fresh desktop's first write, then a start | The stored `agents` block is read; the default workspace is made all the same | `the_default_workspace_is_made_whenever_it_is_the_one_configured`, `a_write_never_stores_the_managed_server_and_starts_a_missing_block` |

### Inspecting a stored server

`mcpServers.inspect {name}` starts the stored server under `name` once —
turned on or off, never one not yet saved — outside any conversation, with no
relay and no session token, through `McpServers::open_once(launch)`: launched
as the live set launches it (`LaunchSettings`), on the live set's SDK client —
which the gateway stops only after it has stopped and drained the
inspections, so a stop past the drain's bound still ends one — never kept
for `tool_ui`, and with no background list. It lists the tools (`McpSession::list_tool_pages`) with
`readOnlyHint` and `destructiveHint` as the server gave them, reads each
distinct UI resource's CSP and permissions as `mcp.readResource` does, then
closes the session, which kills the process group, before it answers:

`{complete, cut?, tools: [{name, readOnlyHint?, destructiveHint?, ui?: {uri,
csp, permissions}}]}`, `csp` and `permissions` in `mcp.readResource`'s shapes.

Its bounds are published in the schema (`x-mcpServerInspect`) and read as
generated constants: one deadline (30 s) on the injected gateway clock from
before the launch until the close, past which the reading — or the close —
is dropped and the process group killed at once, not left the two seconds'
grace a server asked to exit gets; shutdown's stop is observed beside it at
each step the same way; at most 8 pages of tools; at most 32 distinct UI
reads; at most 2 inspections at once, a third answered `mcp_servers_busy`.
`cut` names the first bound that stopped the reading early — `tools`, `ui`,
`stopping` (shutdown, after the server was started; no tools) — or `bytes`,
when the answer would pass the frame's 64 KiB and tools were
dropped from the end until it fits (the product layer measures the frame
it writes). The client waits the deadline plus its allowance.

It asks for `credential.manage`, as the other methods do. It is audited in
`…/audit/mcp-servers` with `action: "inspect"`: it runs an executable the
admin chose with the server's variables, credentials among them, and no
conversation records it. `requested` (target, the revision the server was
read at, and the server as stored — `{name, command, args, enabled,
envNames}`, what ran) is written before the
launch — when it cannot be, nothing is started — and the outcome
(`inspected` with the tool count and cut, or `failed` with the reason and
whether the server was `started`) after
the server has stopped; when that cannot be written, `audit_unavailable` says
whether the server was started, with the failure's `code`. What starts
nothing is not audited: `nessa` (`mcp_servers_reserved_name`), an unknown
name (`mcp_servers_not_found`), no free slot, an unreadable file, a gateway
already stopping (`mcp_servers_stopping`).

Failures: `mcp_servers_invalid` with `{problem, server, name?}` (the stored
server breaks the SDK's rules for what a server is started with — an entry
added to the file by hand — so the SDK refused it before launching: recorded
`failed`, `invalid`, `started: false`), `mcp_server_start_failed` (could not
launch), `mcp_servers_stopping`
(shutdown began before it was started, or the SDK refused to start it
because its client is stopping: recorded `stopping`, `started: false` —
never `gone`), `mcp_server_timed_out` (the
deadline, or a request's own budget), `mcp_server_gone` (it ended, or its
open session was closed by the stop), `mcp_server_malformed` (not MCP, or a UI that is not an
MCP App within its bounds), `mcp_server_remote_error` (a JSON-RPC error,
details `McpRemoteErrorDetails`). Any failure ends the inspection: a partial
answer is only ever one a bound cut.

| # | State / event | Expected | Test |
| --- | --- | --- | --- |
| I1 | The command is missing | `mcp_server_start_failed` | `i1_a_missing_command_fails_to_start` |
| I-invalid | Inspect a stored server that fails validation | `mcp_servers_invalid {problem, server, name?}`; audit `invalid`, not started; nothing launched | `i_invalid_a_stored_server_that_breaks_a_rule_is_invalid_and_not_started`, `an_invalid_stored_server_inspected_is_invalid_and_recorded_not_started`, `mcp_servers_inspect_answers_typed_tools_and_typed_failures` |
| I1a | The SDK client is stopping | `mcp_servers_stopping`, recorded `stopping` with the server not started; nothing launched | `an_inspection_once_the_servers_stop_is_refused_as_stopping_and_starts_nothing`, `an_inspection_is_not_started_unaudited_and_keeps_both_causes`, `mcp_servers_inspect_answers_typed_tools_and_typed_failures` |
| I2 | It never answers `initialize` | `mcp_server_timed_out` at the deadline on a manual clock, not before; its process group — the server and its child — killed | `i2_a_server_that_never_initializes_times_out_and_its_group_is_killed` |
| I2a | It answers `initialize`, then never answers `tools/list` and ignores its stdin closing | `mcp_server_timed_out` at the deadline; killed with its group at once, the slot free within a small real margin | `a_server_that_hangs_after_initialize_is_killed_at_the_deadline` |
| I3 | It exits while listed | `mcp_server_gone` | `i3_a_server_that_exits_mid_list_is_gone` |
| I4 | Tools or UIs past their caps | `complete: false`, `cut` `tools` or `ui`; within both, complete | `i4_tools_or_apps_past_their_caps_are_cut_and_named` |
| I5 | The answer would pass 64 KiB | Tools dropped from the end until it fits; `complete: false`, `cut: "bytes"` unless a bound cut it first. Exactly 65536 bytes is sent whole | `i5_an_answer_past_the_frame_bound_drops_tools_until_it_fits`, `i5_the_frame_bound_holds_at_exactly_its_edge` |
| I6 | A third inspection at once | `mcp_servers_busy`; nothing started | `i6_a_third_inspection_at_once_is_busy` |
| I7 | An unknown name, or `nessa` | `mcp_servers_not_found`, `mcp_servers_reserved_name`; nothing started or audited | `i7_an_unknown_or_managed_name_starts_nothing_and_records_nothing` |
| — | A server that answers | Its tools, hints and apps' CSP and permissions; stopped before the answer | `an_inspection_lists_hints_and_apps_then_stops_the_server`, `mcp_servers_inspect_answers_typed_tools_and_typed_failures` |
| — | The audit | `requested` before the launch, the outcome after the stop; an unwritable `requested` starts nothing; an unwritable outcome keeps both causes | `an_inspection_starts_a_stored_server_on_or_off_and_is_audited_both_sides`, `an_inspection_is_not_started_unaudited_and_keeps_both_causes`, `composed_settings_publish_privately_under_the_lock_and_audit_without_values` |
| I8 | Shutdown's stop while it is opened or read | `cut: stopping`, no tools — on the wire `{complete: false, cut: "stopping", tools: []}`; killed with its group at once, well before the deadline | `a_stop_mid_read_cuts_the_inspection_and_kills_its_group`, `shutdown_cuts_an_inspection_blocked_mid_read_and_records_it_before_the_stop`, `mcp_servers_inspect_answers_typed_tools_and_typed_failures` |
| I9 | Shutdown's stop before it is launched | `mcp_servers_stopping`, nothing launched, no launch begun | `a_stop_given_before_the_launch_starts_nothing` |
| I9a | Shutdown's stop lands as the opening begins | The opening is polled first: a server the SDK refuses before launching answers that refusal, never `cut: stopping` | `a_stop_landing_as_the_opening_begins_never_cuts_a_server_never_started` |
| I10 | The deadline, or the stop, while a server that ignores its stdin closing is being closed after a complete reading | Killed with its group at once, not after the SDK's grace; the reading answered | `the_close_ends_at_the_deadline_or_the_stop` |
| — | `open_once` | No SDK session, no background list, nothing for `tool_ui`; an invalid launch refused before launching; ended by `stop` | `a_session_opened_once_belongs_to_no_conversation_and_stops_with_the_servers`, `listing_tool_pages_stops_at_its_bound_and_says_there_was_more` |

## MCP servers and the restoration identity

This section is the one statement of what the restoration identity means for
MCP servers; other documents link here. The SDK's restoration fingerprint does
not hash the MCP servers. Its inputs are listed once, on `fingerprint` in
[`acp/sessions/identity.rs`](../../crates/nessa-sdk/src/infrastructure/acp/sessions/identity.rs)
(#391, [ADR 344](../adr/todo/344-mcp-ui.md)).

- **A per-open attachment.** The server list — for the gateway, its stand-ins
  — is given to each provider open, like the session token, and selects no
  provider context. Adding, editing or removing a server, or a stand-in's
  command (the gateway's executable) moving, keeps a saved conversation's
  identity, and the conversation resumes with the current list.
- **Read at each provider open.** A provider open reads the live set and
  keeps it for that provider session's life. `mcpServers.save` and `.remove`
  replace the live set, so they reach the next open
  ([the live server set](#the-live-server-set-391)). A restart ends every
  provider session, and a stand-in from an earlier run is refused
  `unknown-session`, since grants are held in memory.
- **`configuration-changed` guards live changes.** A stand-in's arguments carry
  the server's name and a digest of its configured command, arguments and
  environment, keyed per gateway process, which the relay compares with the
  server in the live set at each hello (the Hello rows of
  [a stand-in](#a-stand-in)). Settings apply live
  ([the live server set](#the-live-server-set-391)), so a server changed
  under an open conversation is refused `configuration-changed`, one removed
  `unknown-server`, and that conversation keeps its harness's set until its
  provider session ends.
- **The token stays out of the arguments** because a process list shows a
  process's arguments to every user, and its environment only to its own
  user; the token is in the stand-in's environment instead, where the
  gateway's own user can still read it (what the token keeps apart, above).
- **The one-time strand.** The earlier fingerprint hashed the number of
  servers, even when it was zero, and each server's name, command and
  arguments; for a stand-in the command is the gateway's executable. Dropping
  them changes every saved identity, so the release that ships this answers
  `conversation_configuration_changed` for conversations saved before it.
  Nothing moves them, and no reader of the earlier fingerprint is kept (one
  current contract). The same release re-runs the agent warm-up once, since
  the configuration part of the warm-up's key is this fingerprint
  ([`RuntimeFingerprint`](../../crates/nessa-server/src/agent_warm_up/domain/value_objects/runtime_fingerprint.rs));
  that costs one background launch and is harmless.
- **What still strands a saved conversation.** A change to any hashed input,
  and that includes every update that changes the staged runtime tree, which
  in practice is every release. The desktop stages the runtime under a
  directory named by the prepared tree's content fingerprint
  ([`staging.rs`](../../src-tauri/src/gateway/infrastructure/macos/staging.rs), `tree_fingerprint`)
  and launches the agent runtime's executable and entry from it (`configure`
  in [`composition/desktop.rs`](../../crates/nessa-server/src/composition/desktop.rs)),
  so the executable path and the first argument move with every such update.
  What stops stranding a conversation after this release is only a change to
  the MCP servers, and with it the gateway's own path.
