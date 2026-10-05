# 344. Nessa supports MCP UI: it hosts MCP Apps, and ships its own views as extensions

## Purpose

MCP servers can ship interactive UI through an open extension to MCP, MCP Apps
(`io.modelcontextprotocol/ui`, draft SEP-1865), which ChatGPT's plugins and
other hosts implement. This record makes Nessa one of those hosts. It also
decides that Nessa's own research views — experiments first — are built the
same way, as extensions in
[nessalabs/nessa-extensions](https://github.com/nessalabs/nessa-extensions),
not as code in the desktop app. The widget host of
[326](326-widgets.md) is where they appear.

- **Date:** 2026-09-30
- **Status:** proposed

## Context

MCP Apps works like this:

- A tool names its UI in `_meta.ui.resourceUri`: a `ui://` resource of type
  `text/html;profile=mcp-app`, which the host reads with `resources/read`, with
  the resource's `_meta.ui` giving its CSP domains, permissions and origin.
- The host renders it in a sandboxed iframe behind a proxy on another origin.
  The iframe and the host speak `ui/*` JSON-RPC over `postMessage`: a
  handshake, the tool's input and result, `tools/call` and `resources/read`
  back through the host, messages and model context for the conversation,
  display-mode requests (`inline`, `fullscreen`, `pip`), and host context
  (theme, size, locale, safe area).
- Hosts and servers negotiate it under `capabilities.extensions`, an object
  keyed by the extension's identifier: `{ "io.modelcontextprotocol/ui": {
  mimeTypes: ["text/html;profile=mcp-app"] } }`. OpenAI adds optional `openai/*`
  fields.

Sources: [the MCP extensions
spec](https://github.com/openai/mcp-extensions/blob/main/docs/spec.md),
[building extensions](https://developers.openai.com/plugins/build/extensions).

Nessa today cannot host one (a survey of `main`, recorded in #345):

- **Nessa is not an MCP client.** Each agent harness owns its MCP connections;
  Nessa passes trusted stdio configs in ACP `session/new`
  (`acp/sessions/config.rs`).
- **The ACP tool parser drops `_meta`** and non-text results
  (`acp/tools/wire.rs`), and no layer above it carries a tool's identity,
  `_meta` or structured result.
- **No gateway method** reads a resource or calls a tool for the window.

What binds:

- **The core stays small and robust.** Views like experiments change often and
  should ship, version and fail without the app.
- **A standard beats a private contract.** An extension written once works in
  Nessa and in any other MCP Apps host, and any MCP server with UI works in
  Nessa.
- **Untrusted code is sandboxed.** An app is someone else's HTML; it reaches
  nothing but what the bridge gives it.

## Decision

**Nessa is an MCP Apps host.** An app's `tools/call` and `resources/read` must
reach the same MCP session that produced its tool's result — a stateful server
may have handed back a handle only that session knows — so there is **one
connection per server for each harness session, owned by the gateway**. Per
harness session rather than per server, because one connection shared by
every conversation would let one conversation's state, slow calls and crashes
reach the others (#346). For a configured server, the harness is given a stdio
stand-in (in the server's place in `session/new`); when it starts, the gateway
starts the server and connects to it, and the stand-in forwards the harness's
calls over that connection until the harness session ends. The agent's calls
and its app's calls then travel one upstream session. Through it the gateway lists tools with their `_meta.ui` and
reads `ui://` resources (#346). A tool call's identity, `_meta` and result
travel from the ACP parser to the window's transcript (#347). The gateway offers
the window `mcp.readResource` and `mcp.callTool` for an app, under a policy —
only tools whose `_meta.ui.visibility` includes `"app"`, only on the app's own
server, the person's approval for a destructive tool (below) — with each
call audited (#348). The desktop hosts the app as
an `app`-kind widget (326): a sandbox proxy on a separate origin, a CSP built
only from `_meta.ui.csp` (no network by default), and the `ui/*` bridge mapped
onto the widget host (#349). Nessa declares `capabilities.extensions: {
"io.modelcontextprotocol/ui": { mimeTypes: ["text/html;profile=mcp-app"] } }`;
the `openai/*` fields are optional.

**An app's destructive calls wait on the person, through a review the
gateway owns.** A tool is destructive unless it says it only reads or says
it is not destructive (MCP's own default). Its call waits on a review shown
in the conversation's `permissions` beside the agent's, marked as the app's,
and answered the same way — never the agent's own permission flow, because
the harness did not ask and has nothing waiting. The conversation's approval
mode does not apply: it is trust in the agent, not in an app. The app is not
authenticated beyond the caller's credential, so every step is recorded as
the app's, on that caller's behalf ([design](../../design/mcp-app-calls.md)).

**Display modes map onto 326's places:** `inline` is inline; `fullscreen` on
desktop is a pane beside the conversation, as ChatGPT's desktop draws it; the
window place serves a sidebar entry; `pip` is not offered.

**The sandbox** (#349). The window frames a proxy on an origin of its own —
the `nessa-sandbox` scheme in the desktop app (`http://nessa-sandbox.localhost`
on Windows), a listener beside the dev server in the browser build — with
`allow-scripts allow-same-origin`, as the spec requires of the proxy; the
proxy frames the app's document as `srcdoc` with `allow-scripts` alone, so the
app's origin is opaque. With `allow-same-origin` there too, every app would
share the proxy's origin: one app could script another's proxy through
`top[i]` and drive that app's bridge, and all would share storage. Hosts that
give each app an origin of its own avoid that; a custom scheme and a dev
listener cannot give one per server on every platform, so an app here has no
storage or cookies, and its requests carry `Origin: null`. An origin per
server is the change that would allow more. The CSP is written from the
parsed parts of `_meta.ui.csp` alone (with `form-action 'none'` beside the
spec's directives); nothing declared means no network. The proxy applies it
to its own document before it makes the app's frame — its `frame-src` is what
holds the frame when the app navigates it, which a policy inside the app's
document alone does not (review round 1 on #349 found exactly that escape) —
and the app's document carries it first as well. `frameDomains` is not
applied: `frame-src` is always `'none'`, so the app's frame loads nothing but
its own document — a nested frame loads nothing but inline content, and no
navigation of its own frame goes anywhere — and the app is told so in the
domains it is told were approved. A nested frame needs an origin per app, and Tauri's navigation policy to tell a
frame's load from the window's (it would hand a declared frame to the
person's browser); round 2 on #349 showed what a declared frame lets an app's
frame become. When the app's document goes anyway — a navigation refused, a
reload, a rewritten document — the proxy knows it by which document the
frame holds, not by its `load` events alone (WebKit fires one for a move to
a fragment, and both for going back across one): the reporter, first in
every document, mints an id the document cannot read and says it at once
and in answer to each numbered check the browser delivers from the proxy
(never one the app dispatches), with the token the proxy wrote into the
reporter alone.
The proxy pins the first; another named, or a `load` of the frame whose
latest check that one does not answer within the host's initialize
deadline, is the app's departure. The proxy then stops relaying, removes
the frame and tells the host, which fails the view. The reporter's word on
`pagehide` says it sooner while the app leaves it in place; the guarantee
is not its (round 3 on #349 erased it with `document.open()`). An
`about:srcdoc` document resolves a link to a fragment against the proxy's
URL, which the policy refuses, so the reporter keeps such links in their
document. Permissions (`camera`, …) and
`_meta.ui.domain` are not granted.

What the sandbox does not hold, and is not claimed to: CSP does not govern
WebRTC, so an app can reach a STUN or TURN host it did not declare; a server
may declare a loopback domain (`http://127.0.0.1:…`), as the spec allows for
an app in development, and its app then reaches that local service, with
`Origin: null`; and in the desktop app, whether WebKit refuses an app's
navigation of its own frame (by the proxy's `frame-src`) before Tauri's
navigation policy would hand the URL to the person's browser is not yet seen
(`src-tauri/src/links.rs`). The app speaks for itself: a document it opens
and never closes fires no `load`, so is never checked, nor is one that holds
its own first load back; and a move to
a fragment by its own script (`location.href = "#x"`, `location.assign`)
resolves against the proxy's URL like a link, which the reporter cannot
intercept, and ends its view (an app sets `location.hash`).

**Teardown.** The host sends `ui/resource-teardown` and waits for the answer
(or a deadline) where it ends an app itself and the place stays: an app in a
pane or the window asking to go. When a person closes the place — a pane's
close, ⌘W, the window left — the frame goes with it at once: an iframe's
document is discarded when it leaves the page and reloaded if it is moved, so
nothing sent then could be delivered, and none is sent. That is the one place
the host does not do what the spec says it SHOULD (wait for the answer);
honouring it would mean holding frames outside the places that draw them.

**Nessa's own views ship as extensions.** A research view such as experiments
is an MCP server with an MCP App in nessa-extensions, held to nessa-agent's
coding standards, published separately, and usable in any MCP Apps host. It
gets its data only through the bridge: its tool's result, and `tools/call` to
its own server. [333](333-experiments.md)'s decisions about the experiment
itself — the definition, validation, the one formatter, `bestSoFar` — stand,
and are built there (nessa-extensions #4–#7).

**What stays in the core** is what is about the conversation itself: its
subagents ([329](329-subagents.md)) and the widget host that draws native and
app widgets alike.

## Alternatives considered

- **Experiments as a desktop vertical, MCP UI later.** The earlier plan.
  Simpler to start, but every such view would live in the app, and moving them
  out later would mean two contracts for one job.
- **A private plugin API for Nessa's own views.** Faster to shape, but only
  Nessa could run them, and third-party MCP servers with UI would still need
  MCP Apps.
- **Rendering app HTML without a separate origin.** Simpler in Tauri, but an
  app could then reach the host's storage and APIs; the standard requires the
  proxy.

## Consequences

- The gateway gains the connection to each server for each harness session —
  the harness's MCP traffic now passes through it — and two app methods, with
  policy and audit; what that connection means is designed in
  [mcp-connections](../../design/mcp-connections.md) (#346): a stand-in's
  arguments carry a digest of the configured server, which the relay compares;
  a gateway restart ends every session, so a restored
  conversation's handles are gone and the server says so; a server that exits
  ends its stand-in, as when the harness owned it; and which conversation a
  stand-in belongs to is carried to the gateway by a token issued for each
  open, in the stand-in's environment (#348), before any app method exists.
  The SDK and protocol carry tool identity and `_meta`, which also helps any
  tool view in the transcript.
- **Amended (#391): the MCP server list is not part of a conversation's
  restoration identity.** It is attached to each open, like the stand-in
  token, and selects no provider context, so the SDK's fingerprint no longer
  hashes it. This replaces "the fingerprint still changes exactly when the
  server does", which the record first chose. The consequences — the one-time
  strand of conversations saved before the release, what still strands one,
  and what `configuration-changed` is for — are in
  [MCP servers and the restoration identity](../../design/mcp-connections.md#mcp-servers-and-the-restoration-identity).
- A view like experiments becomes a package with its own release, testable in a
  fake host, and portable.
- An extension cannot reach into the core: whatever it needs from the
  conversation must come through the bridge. An experiment's agents appearing
  as the conversation's subagents (#337) needs such a seam, decided once #347
  lands.
- Remaining, each its own record when it comes: a plugin manifest bundling
  skills, MCP servers and apps (with ADR 0012); installing extensions from a
  directory; the `openai/*` extensions Nessa chooses to support.
- Work: #346, #347, #348, #349 in nessa-agent; nessa-extensions #1–#7. Part of
  #345.

## Evidence (#349)

- **The bridge**, in jsdom, through the real frame transport and the spec's
  own messages: `widgets/app/application/bridge.test.ts`, one test at least
  per row and ordering of the design table on #349; the parsing, the CSP, the
  places, the host context, the lifecycle and the tool call's notifications
  each under `widgets/app/model/`; the hosts drawing an app,
  `widgets/app/ui/app-view.test.tsx`.
- **In a real browser**: `verification/desktop/scripts/mcp-apps.mjs`, with
  the fixture app (`widgets/app/fixture/`) — it renders inline, in a pane and
  in the window; `tools/call` is answered, and refused for a hidden tool; a
  request its CSP does not declare is blocked; its origin is opaque; it is
  torn down on close (`verification/desktop/CHECKLIST.md` › _MCP Apps_).
- **The desktop app's scheme** serves the proxy and nothing else
  (`src-tauri/src/app_sandbox.rs`), and the navigation policy lets the proxy
  and the app's `srcdoc` load in a frame (`links.rs`). Not driven by a script:
  the scripts run the browser build.

## Evidence (#384)

- **The adapter** (`widgets/app/adapters/gateway/mcp-app-server.ts`), against
  a fake `client.mcpApps` that gives every answer the gateway can:
  `mcp-app-server.test.ts`, one test at least per row of the state table on
  #384 — each refusal with its reason, `server-gone` and `failed` apart, a
  server's own JSON-RPC error passed on with its signed code, what the app
  sends held to the client's bounds (`mcpAppRequestProblem`) before sending,
  the ticket redeemed once and never handed on — and
  #349's L14 and L24 through the real bridge over it.
- **The mount**: each view mints its `instanceId` and releases it once, the
  first time it fails or ends, aborting what its reads have not fetched
  (`bridge.test.ts`, "the mount and its release"; `app-view.test.tsx` under
  StrictMode). A first read the gateway was too busy for is made again
  (`bridge.test.ts`, L1b).
- **Limits, each its own issue**: a release ends the reviews already open, but
  a call admitted before it can still open one after (#397); the app lane's
  4 slots per socket are shared by every app in the window (#398); an app is
  told `{}` for arguments the view does not carry (#394); a frame the gateway
  cannot decode is answered `invalid_request` when the envelope parser reads
  one JSON object, no decoded envelope name appears twice, `type` is `req`,
  and `id` is one Unicode string of 1 to 256 bytes. An envelope name that is
  not Unicode is not a second name, a repeated name inside a nested value
  still leaves that id, and a frame deeper than 127 containers is not read
  (#403),
  so the client refuses a lone surrogate in what it sends; and it closes the
  socket on a frame past its
  message limit, so the client refuses one before sending
  (`NessaRequestTooLargeError`).
- **The calls from the transcript** (`app-calls.ts`), each named by its
  conversation as well as its execution and tool ids, and kept, at the last
  state a view reported, until the conversation is deleted — a view holds only
  its latest tools — and a forgotten conversation is not brought back by a
  late view. The order of views is the gateway source's to keep: it tells
  the apps each view in the order read, and a conversation the gateway says
  was deleted (`gateway-source.test.ts`, "MCP Apps (#384)"), and its apps'
  calls go on the client it holds (`dependencies.test.ts`):
  `app-calls.test.ts`, and
  `workspace/adapters/gateway/tool-widget.test.ts` for the widget the
  transcript draws reading the same call.
- **Not yet in a real browser against a real gateway**: the gateway source
  (#248) is on `main`, so the window draws a real conversation's apps in a
  browser preview opened with `?gateway`; the Chromium and WebKit run against
  a real server is still to do.

## What each harness passes through ACP

A spike for #347, first read from the pinned harnesses' bundled code and then
**observed** in live turns: a real gateway, the real harness, a real model, and
the test MCP server `scripts/mcp-test-server/` configured as `mcptest`, with
every ACP frame and every MCP frame recorded (`live-check.mjs`, 2026-10-01).
Claude ran `@agentclientprotocol/claude-agent-acp` 0.76.0 on `claude-sonnet-5`;
Codex ran `@agentclientprotocol/codex-acp` 1.12.0 (codex 0.154.0) on
`gpt-5.6-terra`. Opencode 1.18.31 could not be run: the gateway refuses it
without an OpenCode credential, and this machine has none, so its column is
still read from its compiled bundle. The recorded frames are the SDK parser
tests' fixtures (`tests/infrastructure/{claude_acp,codex_acp}/tools/fixtures/`).

| | Claude ACP 0.76.0 (observed) | Codex ACP 1.12.0 (observed) | Opencode 1.18.31 (from bundled code) |
| --- | --- | --- | --- |
| Server and tool | `_meta.claudeCode.toolName` is `mcp__<server>__<tool>` on every frame, and `title` on the announcement and the frames that restate input, each name with `[^A-Za-z0-9_-]` replaced by `_`: `rows.get` arrives as `rows_get`; kind `other`. Only a configured prefix says where the server ends: Nessa's server names hold no `__` but may end in `_`. In the run, each MCP call was preceded by a `ToolSearch` call that loaded its schema (the fixture keeps the first) | `rawInput.{server, tool}` exactly (`rows.get` kept); title `mcp.<server>.<tool>`, kind `execute`; `_meta.is_mcp_tool_call` on the announcement only, which is `in_progress`; a bare `{status: in_progress}` update follows; the completion repeats `rawInput` without the marker | title `<server>_<tool>` after the same replacement, kind `other`, no `_meta`: cannot be split |
| The tool's `_meta` (`ui.resourceUri`) | **not passed on**: no `ui://` or `resourceUri` in any frame | **not passed on**: no `ui://` or `resourceUri` in any frame | not passed on |
| The result's `_meta` | not in any `session/update` | `rawOutput.result._meta` (`null` when the result has none) | dropped |
| `structuredContent` | replaces the result's text as JSON text, in `content`, `rawOutput`, and a separate update's `_meta.claudeCode.toolResponse`: indistinguishable from text | `rawOutput.result.structuredContent`, verbatim (`null` when absent) | JSON text only when there is no other content |
| resource, resource_link | turned into text: `[Resource link: <name>] <uri>`, `[Resource from <server> at <uri>] <text>`; never an ACP `resource` or `resource_link` block | verbatim inside `rawOutput.result.content`; no ACP content at all for an MCP call | resource text becomes text; links dropped |
| `isError` | the call ends `failed`; `rawOutput` is the error text | the call ends `failed`; `rawOutput.result` has no `isError` field | — |
| `rawOutput` | the result text (a string), or the text blocks | `{result: {content, structuredContent, _meta} \| null, error: {message} \| null}` | `{output, metadata?, attachments?}` |

Seen once, not explained: in two of three Codex runs the model answered that
the `mcptest` tools were not available, though codex had started the server;
the third run, with the server's traffic recorded, called all five. Codex run
directly (`codex exec`) behaved the same way once. Unverified: Opencode, live.

What #347 builds on it: the SDK carries an MCP call's server and tool
(`McpTool`) from Claude, split at the one configured prefix that fits (its tool
name in the harness's replaced spelling), and from Codex's `rawInput` exactly,
and names none for Opencode; it carries a Codex MCP result
— its text blocks, and its `structuredContent` bounded as a structured result —
where before a finished Codex MCP call showed nothing. Only Codex's announcement
carries the MCP marker; its completion, which carries the result, does not
(`index.js:25123-25130`), so the adapter remembers the marking per call. Both reach the window on
`ConversationTool` (`mcp`, `structuredContent`).
Claude's `structuredContent`, which no ACP frame carries as an object, is taken from the
gateway's own connection instead: its stand-in keeps each forwarded result
for the call it was reported under (#435, [forwarded results](../../design/mcp-connections.md#forwarded-results)).

What it does not: **no harness passes the tool's UI resource through ACP**, so a
tool's `_meta.ui.resourceUri` comes from the server itself, over the gateway's
own connection (#346) — the connection this record already decides the gateway
owns. Until then no call is known to have UI, and the desktop's widget part
(`toolWidget` in `workspace/model/transcript.ts`) is never produced; a call's
text result is shown as before.
