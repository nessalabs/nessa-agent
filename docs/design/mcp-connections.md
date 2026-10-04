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
  server's configured command and arguments.
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
  arguments still carry the server's name and a digest of the configured
  command and arguments, which the relay compares: a server changed under an
  open conversation is refused `configuration-changed`, and that conversation
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

## MCP servers leave the restoration identity (#391)

Removing the server loop from the fingerprint changes every saved
conversation's identity once: the old hash wrote the number of servers even
when it was zero, and each server's command, which for a stand-in is the
gateway's executable path. The release that ships this therefore strands the
conversations saved before it once — they answer
`conversation_configuration_changed`, as every app update already does today,
because each version runs its gateway from a new directory. Nothing moves them
and no reader of the earlier fingerprint is kept (one current contract). From
that release on, neither an MCP server change nor a move of the gateway's
executable strands a saved conversation; only the inputs that still select the
provider's context do.
