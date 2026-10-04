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
`session/resume` and for all three profiles. It is never in the arguments,
so it stays out of the context fingerprint, like credentials. The relay
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
  id, verbatim — except that a `tools/list` answer leaves out a tool whose
  `_meta.ui.visibility` does not include `model` (a `visibility` that is not
  an array of strings counts as leaving it out), and a `tools/call` naming a
  tool hidden so is refused (`-32602`) — hidden by the session's own latest
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
  (`model`, `app`; both when absent, or when there is no `_meta.ui`), each
  read on its own. A tool whose `resourceUri` is absent or cannot be read is
  kept without a UI, and keeps its `visibility` (#412); one whose
  `visibility` is not an array of strings (`null` included) is no one's —
  hidden from the model, and an app's `tools/call` to it is refused
  `tool_not_for_app` — and keeps a readable `resourceUri` as its UI. A tool
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
| Serving | `tools/list` answer | Serving | forwarded without the tools whose visibility excludes the model; of two lists answered out of order, the one asked later decides |
| Serving | `tools/call` for a tool the session's latest list, or the stand-in's, hid — or any tool before a list has said anything — | Serving | refused `-32602`, nothing forwarded; a name a list gives twice is hidden if either says so |
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

- **Context fingerprint.** The SDK fingerprints what the harness is launched
  with, but not its MCP servers (#391, ADR 344): the server list is a per-open
  attachment, like the token, and not a selector of the provider's context.
  Adding, editing or removing a server, or moving the gateway's executable or
  the relay socket, leaves every saved conversation restorable. A stand-in's
  arguments still carry the server's name and a keyed digest of the
  configured command, arguments and environment, which the relay compares against the live set at
  each hello: a server changed under an open conversation is refused
  `configuration-changed`, one removed `unknown-server`, and that conversation
  keeps its harness's set until its provider session ends. The session token
  is in the stand-in's environment, never its arguments.
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
  UI, unreadable `_meta.ui`, list bounds and order, read of an app resource,
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
  server list left out of the fingerprint
  (`adding_an_mcp_server_keeps_the_identity_and_restores`).
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
it honest to keep the server list out of the restoration identity.

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
stand-in, and the restoration identity reads none of it.

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

`nessa` is Nessa's own server (`MANAGED_SERVER_NAME`). The desktop drops any
stored `nessa` and adds the bundled one in memory; the gateway takes it from
the startup configuration into `LaunchSettings`, lists it `managed`, and
refuses every save or remove naming it (`mcp_servers_reserved_name`). A write
edits the file, not the composed configuration, so the file never gains it.

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
  `envNames` sorted by name, as the variables are stored.
- `mcpServers.save {revision, previousName?, server: {kind: "stdio", name,
  command, args, env: [{name, value | null}], enabled}}` → `{revision}`. The
  server's `env` is exactly the names listed; `value: null` keeps the value
  stored for that name on the server being saved (the one under
  `previousName`, else under `name`).
- `mcpServers.remove {revision, name}` → `{revision}`.

The revision is a digest of the stored block (`[]` when there is none),
keyed with the process's `ConfigurationKey` — the one the stand-ins' digests
are keyed with (`stored_revision`) — so it gives nothing to test a guessed
variable value against
(`the_revision_is_keyed_and_changes_with_a_variables_value`). It changes
across a restart, which costs a caller holding one from before it one
conflict. Nothing beside it is persisted. Each change, in order:

1. The `requested` record (`…/conversations/audit/mcp-servers`), with the
   target, the caller's revision, the variable names and the initiator. When
   it cannot be written: `audit_unavailable` (`applied: false`), and nothing
   is locked, written or applied.
2. `config.json.lock`, tried every 20 ms for at most 2 s on the gateway
   clock; still held: `mcp_servers_busy`.
3. Read the file, checked by the runtime configuration's own parse and its
   64 KiB bound (`RuntimeConfig::parse`, `MAX_CONFIG_BYTES`); compare the
   revision; make the edit; check the result with the SDK's rules
   (`McpServerLaunch::problem_in`, managed server included).
4. Re-read the file and compare its revision with the one the edit was made
   to — an edit made outside the lock since the read is
   `mcp_servers_revision_conflict` with the revision now, never overwritten
   (`a_change_made_outside_the_lock_after_the_read_is_a_conflict`). Write the
   whole file with only the block replaced, check it again, and publish it in
   one step, private (0600). The lock travels into each blocking read and
   write and back out, so a caller gone mid-step leaves it held until that
   step has finished
   (`a_caller_gone_mid_write_leaves_the_lock_held_until_the_write_ends`). The whole file is re-serialised:
   the gateway owns `config.json`, so its key order and layout after a write
   are the gateway's, and everything else in it keeps its value, not its
   spelling. A file with no `agents` block gains one from the running catalog
   and workspace; the desktop makes its default workspace whenever that is
   the one configured, so the next start does the same with the block as
   without it.
5. Replace the live set, still under the lock, so changes publish and
   replace in the same order. Refused only once the gateway is stopping: the
   change still answers success, the outcome says `liveSetReplaced: false`,
   and the next start reads the file.
6. Unlock, then the outcome record: `applied` with the revision and names
   before and after, or `refused`/`failed` with the reason and what was
   stored when it was read. Each side names the target as stored there —
   `{name, command, args, enabled, envNames}`, before under `previousName`
   for a rename, `null` where none is stored
   (`the_audit_records_the_targets_before_and_after_on_save_rename_disable_and_remove`). When it cannot be written: `audit_unavailable`
   with whether the change applied and, when it did not, the `code` it
   would have been answered with, so neither cause is lost. Nothing is
   rolled back.

A record names servers and variables, never a variable's value; so do the
wire and every `Debug` (`ConfiguredMcpServer`, `ServerSave`,
`McpServerLaunch`). The file is never repaired: one that does not parse is
refused `mcp_servers_config_invalid` before and after the edit.

Errors are `McpServersErrorCode`: `mcp_servers_not_configured`,
`mcp_servers_invalid` (details `{problem, name?}`), `mcp_servers_reserved_name`,
`mcp_servers_not_found`, `mcp_servers_revision_conflict` (details
`{revision}`), `mcp_servers_busy`, `mcp_servers_config_invalid`,
`mcp_servers_config_too_large`, `mcp_servers_storage_unavailable`,
`audit_unavailable` (details `{applied, code?}`), and the inspection codes
below. `mcp_servers_not_configured` means
this gateway holds no live set to manage: not Unix, no agents configured, or
MCP off this run because the relay socket could not be bound (or a path is
not UTF-8). A gateway that holds one always manages it, with or without
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
| LS3 | Two saves at one revision at once | The lock serialises them; the first wins, the second gets LS2 | `s3_two_saves_at_one_revision_are_serialised_and_the_second_conflicts`, `s3_two_saves_at_one_revision_over_the_real_lock_are_serialised` |
| LS3a | The file edited outside the lock between a change's read and its write | `revision_conflict` with the revision now; nothing written; the live set kept | `a_change_made_outside_the_lock_after_the_read_is_a_conflict` |
| LS3b | The caller goes away while the write is under way | The lock stays held until the write has finished | `a_caller_gone_mid_write_leaves_the_lock_held_until_the_write_ends` |
| LS4 | Lock held past its bound | `busy`; nothing written | `s4_a_lock_held_past_its_bound_is_busy_and_nothing_is_written`, `composed_settings_publish_privately_under_the_lock_and_audit_without_values` |
| LS5 | Publish fails | `storage_unavailable`; the old file and live set kept; outcome `failed` | `s5_a_failed_publish_keeps_the_old_file_and_live_set` |
| LS6 | `requested` can't be written | `audit_unavailable`; no lock, no write, no apply | `s6_an_unwritable_requested_record_stops_everything` |
| LS7 | Published, then the outcome fails | `audit_unavailable` with `applied: true`; the file and live set are new. Refused or failed, then the outcome fails: `applied: false` with the refusal's `code` | `s7_an_unwritable_outcome_after_a_publish_says_it_applied`, `mcp_servers_on_the_wire_carry_names_only_and_typed_refusals` |
| LS8 | `config.json` doesn't parse, before or after the edit | `config_invalid`; nothing written, nothing repaired | `s8_a_configuration_that_does_not_parse_is_refused_and_never_repaired` |
| LS9 | The result would pass 64 KiB | `config_too_large`; nothing written. Exactly 65536 bytes is read and written; 65537 is refused | `s9_a_result_past_the_bound_is_refused_and_nothing_is_written`, `the_configuration_bound_holds_at_exactly_its_edge` |
| LS10 | A 17th server, a bad name, a duplicate, `nessa`, a bad or reserved variable name | `invalid` with the typed problem, or `reserved_name`; nothing written | `s10_an_invalid_or_reserved_server_is_refused_with_its_problem`, `no_edit_names_the_managed_server` |
| LS11 | A server edited while a conversation's harness has it — its command, arguments, or only its variables | The running harness and its server process are untouched (a relaunch of the same provider session keeps its set); its stand-in's next hello is refused `configuration-changed`; the next open gets the new stand-in | `a_replaced_set_refuses_old_stand_ins_and_leaves_running_ones_alone`, `s11_to_s13_a_replaced_set_reaches_the_next_open_and_old_stand_ins_are_refused`, `each_open_reads_the_hosts_servers_and_keeps_them_through_a_relaunch`, `a_replaced_set_is_read_by_the_next_opening_and_leaves_open_sessions_alone` |
| LS12 | A server removed or turned off while open | Its stand-in's next hello is refused `unknown-server`; a new open does not list it; an open session of it is untouched | the same, `a_disabled_server_stays_stored_and_out_of_the_live_set` |
| LS13 | Added back, or turned on again | It is in the next open, and its stand-ins are let through | the same |
| LS14 | `replace` once stopping, or contending with a `stop` that holds the lock | Refused `Stopped`; the set is kept; nothing launched. A `save` or `remove` that publishes then answers success, outcome `liveSetReplaced: false` | `a_replacement_once_stopping_is_refused_and_launches_nothing`, `a_replacement_contending_with_a_stop_that_holds_the_lock_sees_the_stop`, `a_publish_during_stop_answers_success_and_leaves_the_live_set` |
| LS15 | Rename (`previousName`) | One write: the old name gone, the new one in its place; unknown `previousName` → `not_found` | `s15_a_rename_is_one_write_and_an_unknown_previous_name_is_not_found` |
| LS16 | Remove an unknown name | `not_found`; nothing written | `s16_removing_an_unknown_name_is_not_found` |
| LS17 | `save` keeps a variable with `value: null` | The stored value is kept; a null for a name with no stored value → `invalid` (`environment_value_missing`); an entry with no `value` at all → `invalid_request` | `s17_a_null_value_keeps_the_stored_one_and_needs_one_to_keep`, `mcp_servers_on_the_wire_carry_names_only_and_typed_refusals` |
| LS18 | No servers at startup, then one added | The relay exists; a new open gets the server and its stand-in is let through | `s18_with_no_server_configured_the_relay_exists_and_a_server_added_reaches_the_next_open` |
| — | A replacement that breaks a rule | Refused `InvalidConfiguration` with the problem; the set is kept | `an_invalid_replacement_is_refused_and_keeps_the_set`, `the_sets_count_and_each_servers_environment_are_checked_by_one_owner` |
| — | `replace` lands between a hello's admission and its open | The open is refused as the admission would refuse it now: `configuration-changed` for an edit, `unknown-server` for a removal; nothing launched | `a_replacement_between_admission_and_opening_refuses_the_opening`, `an_opening_admitted_on_a_replaced_configuration_is_refused` |
| — | A variable's value | Never in the wire, `list`, the audit, or a `Debug`; nor in a stand-in's arguments or the revision, raw or as an unkeyed hash | `a_launch_prints_its_environment_names_never_its_values`, `a_configured_server_prints_its_variable_names_never_their_values`, `launch_settings_print_names_never_values`, `the_list_names_each_variable_and_marks_the_managed_server`, `a_stand_ins_arguments_reveal_nothing_about_a_variables_value`, `the_revision_is_keyed_and_changes_with_a_variables_value` |
| — | A stored `nessa` on a gateway without the desktop, on or off | The managed server: listed once with its `enabled` and variable names, counted, launched only when on | `a_stored_nessa_on_a_headless_gateway_is_the_managed_server_on_or_off` |
| — | A server saved while an agent with tools off is composed | That agent is given no MCP servers or grants; its opens and deletes go ahead | `a_tools_disabled_agent_opens_and_deletes_with_a_server_saved` |
| — | A server name starting or ending with `_` | `invalid` (`name`): `mcp__<server>__<tool>` would not say where the server ends | `a_server_name_may_not_start_or_end_with_an_underscore` |
| — | The digest | Changes with the command, each argument, each variable's name and value, and the key | `the_digest_changes_with_the_command_each_argument_each_variable_and_their_boundaries`, `each_server_is_handed_over_as_a_relay_under_its_own_name` |
| — | A fresh desktop's first write, then a start | The stored `agents` block is read; the default workspace is made all the same | `the_default_workspace_is_made_whenever_it_is_the_one_configured`, `a_write_never_stores_the_managed_server_and_starts_a_missing_block` |

### Inspecting a stored server

`mcpServers.inspect {name}` starts the stored server under `name` once —
turned on or off, never one not yet saved — outside any conversation, with no
relay and no session token, through `McpServers::open_once(launch)`: launched
as the live set launches it (`LaunchSettings`), on the live set's SDK client
so a gateway stopping ends it, never kept for `tool_ui`, and with no
background list. It lists the tools (`McpSession::list_tool_pages`) with
`readOnlyHint` and `destructiveHint` as the server gave them, reads each
distinct UI resource's CSP and permissions as `mcp.readResource` does, then
closes the session, which kills the process group, before it answers:

`{complete, cut?, tools: [{name, readOnlyHint?, destructiveHint?, ui?: {uri,
csp, permissions}}]}`, `csp` and `permissions` in `mcp.readResource`'s shapes.

Its bounds are published in the schema (`x-mcpServerInspect`) and read as
generated constants: one deadline (30 s) on the injected gateway clock from
before the launch until the last read, past which the reading is dropped and
the process group killed at once — not closed with the two seconds' grace a
server asked to exit gets; at most 8 pages of tools; at most 32 distinct UI
reads; at most 2 inspections at once, a third answered `mcp_servers_busy`.
`cut` names the first bound that stopped the reading early — `tools`, `ui` —
or `bytes`, when the answer would pass the frame's 64 KiB and tools were
dropped from the end until it fits (the product layer measures the frame
it writes). The client waits the deadline plus its allowance.

It asks for `credential.manage`, as the other methods do. It is audited in
`…/audit/mcp-servers` with `action: "inspect"`: it runs an executable the
admin chose with the server's variables, credentials among them, and no
conversation records it. `requested` (target, the revision the server was
read at, its variable names, whether it is on) is written before the
launch — when it cannot be, nothing is started — and the outcome
(`inspected` with the tool count and cut, or `failed` with the reason) after
the server has stopped; when that cannot be written, `audit_unavailable` says
whether the server was started, with the failure's `code`. What starts
nothing is not audited: `nessa` (`mcp_servers_reserved_name`), an unknown
name (`mcp_servers_not_found`), no free slot, an unreadable file.

Failures: `mcp_server_start_failed` (could not launch), `mcp_server_timed_out`
(the deadline, or a request's own budget), `mcp_server_gone` (it ended, or
the gateway stopped), `mcp_server_malformed` (not MCP, or a UI that is not an
MCP App within its bounds), `mcp_server_remote_error` (a JSON-RPC error,
details `McpRemoteErrorDetails`). Any failure ends the inspection: a partial
answer is only ever one a bound cut.

| # | State / event | Expected | Test |
| --- | --- | --- | --- |
| I1 | The command is missing | `mcp_server_start_failed` | `i1_a_missing_command_fails_to_start` |
| I2 | It never answers `initialize` | `mcp_server_timed_out` at the deadline on a manual clock, not before; its process group — the server and its child — killed | `i2_a_server_that_never_initializes_times_out_and_its_group_is_killed` |
| I2a | It answers `initialize`, then never answers `tools/list` and ignores its stdin closing | `mcp_server_timed_out` at the deadline; killed with its group at once, the slot free within a small real margin | `a_server_that_hangs_after_initialize_is_killed_at_the_deadline` |
| I3 | It exits while listed | `mcp_server_gone` | `i3_a_server_that_exits_mid_list_is_gone` |
| I4 | Tools or UIs past their caps | `complete: false`, `cut` `tools` or `ui`; within both, complete | `i4_tools_or_apps_past_their_caps_are_cut_and_named` |
| I5 | The answer would pass 64 KiB | Tools dropped from the end until it fits; `complete: false`, `cut: "bytes"` unless a bound cut it first. Exactly 65536 bytes is sent whole | `i5_an_answer_past_the_frame_bound_drops_tools_until_it_fits`, `i5_the_frame_bound_holds_at_exactly_its_edge` |
| I6 | A third inspection at once | `mcp_servers_busy`; nothing started | `i6_a_third_inspection_at_once_is_busy` |
| I7 | An unknown name, or `nessa` | `mcp_servers_not_found`, `mcp_servers_reserved_name`; nothing started or audited | `i7_an_unknown_or_managed_name_starts_nothing_and_records_nothing` |
| — | A server that answers | Its tools, hints and apps' CSP and permissions; stopped before the answer | `an_inspection_lists_hints_and_apps_then_stops_the_server`, `mcp_servers_inspect_answers_typed_tools_and_typed_failures` |
| — | The audit | `requested` before the launch, the outcome after the stop; an unwritable `requested` starts nothing; an unwritable outcome keeps both causes | `an_inspection_starts_a_stored_server_on_or_off_and_is_audited_both_sides`, `an_inspection_is_not_started_unaudited_and_keeps_both_causes`, `composed_settings_publish_privately_under_the_lock_and_audit_without_values` |
| — | `open_once` | No SDK session, no background list, nothing for `tool_ui`; an invalid launch refused before launching; ended by `stop` | `a_session_opened_once_belongs_to_no_conversation_and_stops_with_the_servers`, `listing_tool_pages_stops_at_its_bound_and_says_there_was_more` |

## MCP servers leave the restoration identity (#391)

Removing the server loop from the fingerprint changed every saved
conversation's identity once — the old hash wrote the number of servers even
when it was zero. Saved sessions are an append-only, replayed log, so a
snapshot cannot be edited in place; the gateway appends one durable
`SessionChange::ProviderIdentity { before, after }` per conversation instead,
once, as it starts (`conversation::application::identity_retrofit`). It runs
after session storage initialises and the agent resolver exists, before the
conversation service is built, so nothing can hold a session lease yet; the
registry lock already refuses a second gateway. Each conversation's agent,
model and approval mode resolve through the same resolver reopening uses, to
the current identity and the one the same provider had under the old
fingerprint (`previous_identity`); OpenCode is observed under its 5-second
deadline without admitting a warm-up.

Run: `NotRun → Running → Done (marker) | Incomplete (no marker)`.
Conversation: `Unexamined → Rewritten | AlreadyCurrent | Foreign | NoHistory |
Tombstoned | LeftPermanent(reason) | LeftTransient(reason)`. A rewrite is
audited twice under `conversations/audit/fingerprint-retrofit/` — intent
(`rewriting`, before/after identities, conversation, cause
`mcp_servers_left_restoration_identity`, initiator system/gateway start) before
the append and the outcome after it — and every run records a summary of counts
and leftovers. The marker is `conversations/retrofit/391-fingerprint.done`.
The runner, its audit and marker, and the SDK's earlier fingerprint are
temporary; [#471](https://github.com/nessalabs/nessa-agent/issues/471) deletes
them together.

| # | State / event | Expected | Marker | Test |
| --- | --- | --- | --- | --- |
| R1 | Saved provider == old fingerprint over current config | One `ProviderIdentity` unit appended; reopen restores with no `IdentityMismatch`; audit holds the intent, then `rewritten` | yes | `r1_a_conversation_saved_under_the_previous_identity_is_moved_and_reopens` |
| R2 | Saved provider == new fingerprint | No write, counted | yes | `r2_a_conversation_already_current_is_counted_and_not_written` |
| R3 | Matches neither | Untouched, `foreign`; reopen still answers `IdentityMismatch` | yes | `r3_a_conversation_matching_neither_identity_is_left_foreign` |
| R4 | Next start with the marker present | Nothing opened, nothing audited | n/a | `r4_with_the_marker_present_nothing_is_opened_or_audited` |
| R5 | Crash after the unit, before `SaveComplete` | Next start reads the prior publication (still old) and retries the exact move; the writer accepts it | after R1 | `r5_a_move_interrupted_before_its_completion_is_retried_exactly` |
| R6 | Crash after the append, before the marker | Next start: R2 for that conversation, then the marker | yes | `r6_a_run_that_stopped_before_its_marker_finds_the_move_already_current` |
| R7 | `open_existing` answers `Busy`, or a storage I/O error | `LeftTransient`, recorded | **no** | `r7_a_busy_or_failing_history_is_left_for_the_next_start` |
| R8 | Never opened (no session stream) | `NoHistory` | yes | `r8_a_conversation_never_opened_has_no_history` |
| R9 | Log corrupt, wrong stream key, or unfinished and refused | `LeftPermanent(corrupt)`, not repaired | yes | `r9_a_corrupt_or_foreign_keyed_history_is_left_for_good`, `r9_an_unfinished_save_that_is_not_this_move_is_left_for_good` |
| R10 | Agent not configured or installed, model or mode unavailable, unsupported agent | `LeftPermanent(reason)` | yes | `r10_an_unresolvable_selection_is_left_for_good_with_its_reason` |
| R11 | OpenCode resolver times out | `LeftTransient(unavailable)`; other conversations still processed, the selection resolved once | **no** | `r11_a_resolver_timeout_leaves_its_conversations_and_processes_the_rest` |
| R12 | Audit cannot be written before the rewrite | No unit appended, `LeftTransient(audit)` | **no** | `r12_no_move_is_appended_when_its_intent_cannot_be_recorded` |
| R13 | Intent audited, then the append fails | Outcome `left: storage`, log unchanged | **no** | `r13_a_failed_append_after_its_intent_is_recorded_as_left`, `r13_a_finished_save_whose_move_is_refused_as_corrupt_is_left_for_the_next_start` |
| R14 | Conversation tombstoned | Skipped; deletion owns it | yes | `r14_a_deleted_conversation_is_skipped` |
| R15 | Second gateway in the same namespace | Refused earlier by the registry lock; the retrofit never starts | n/a | existing `bootstrap_is_explicit_private_and_exclusively_locked` (`nessa-auth` local registry) |
| R16 | Zero MCP servers configured | Still R1: the old hash included the zero length | yes | `r16_with_no_mcp_servers_the_previous_identity_still_differs` (SDK), `previous_identity_differs_with_no_mcp_servers` |
| R17 | Folding `ProviderIdentity` whose `before` ≠ published, or `before == after` | Refused as corrupt, nothing published | n/a | `a_provider_identity_change_that_does_not_continue_the_published_one_is_refused` |
| R18 | Summary audit cannot be written | No marker; the next start re-runs and finds R2 | **no** | `r18_no_marker_is_written_when_the_summary_cannot_be_recorded` |
| R19 | A conversation's metadata row cannot be read (found while building) | `LeftTransient(metadata)`: the store cannot tell a damaged row from a failed read | **no** | `r19_an_unreadable_metadata_row_is_left_for_the_next_start` |
