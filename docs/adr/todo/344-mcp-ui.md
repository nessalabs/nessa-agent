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
connection per server, and the gateway owns it**. For a configured server, the
gateway starts it and connects to it, and gives the harness a stdio stand-in (in
`nessa-mcp`'s place in `session/new`) that forwards the harness's calls over
that same connection. The agent's calls and the app's calls then travel one
upstream session. Through it the gateway lists tools with their `_meta.ui` and
reads `ui://` resources (#346). A tool call's identity, `_meta` and result
travel from the ACP parser to the window's transcript (#347). The gateway offers
the window `mcp.readResource` and `mcp.callTool` for an app, under a policy —
only tools whose `_meta.ui.visibility` includes `"app"`, only on the app's own
server, approval through the existing permission flow for a tool marked
`destructiveHint` — with each call audited (#348). The desktop hosts the app as
an `app`-kind widget (326): a sandbox proxy on a separate origin, a CSP built
only from `_meta.ui.csp` (no network by default), and the `ui/*` bridge mapped
onto the widget host (#349). Nessa declares `capabilities.extensions: {
"io.modelcontextprotocol/ui": { mimeTypes: ["text/html;profile=mcp-app"] } }`;
the `openai/*` fields are optional.

**Display modes map onto 326's places:** `inline` is inline; `fullscreen` on
desktop is a pane beside the conversation, as ChatGPT's desktop draws it; the
window place serves a sidebar entry; `pip` is not offered.

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

- The gateway gains the one connection to each server — the harness's MCP
  traffic now passes through it — and two app methods, with policy and audit;
  the SDK and protocol carry tool identity and `_meta`, which also helps any
  tool view in the transcript.
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

## What each harness passes through ACP

A spike for #347, read from the pinned harnesses installed with the app —
`@agentclientprotocol/claude-agent-acp` 0.76.0 (and the Claude Code CLI it
drives, 0.3.257), `@agentclientprotocol/codex-acp` 1.12.0 (over codex
0.154.0), and Opencode 1.18.31 — not from a recorded turn: the repository's
MCP frames are hand-written, and none was run against a live server with UI.
Claude's adapter and codex-acp are readable JavaScript and are cited by line;
the Claude CLI, codex and Opencode are compiled, and what is said of them comes
from their embedded strings and bundles.

| | Claude ACP 0.76.0 | Codex ACP 1.12.0 | Opencode 1.18.31 |
| --- | --- | --- | --- |
| Server and tool | `_meta.claudeCode.toolName` (and `title`) is `mcp__<server>__<tool>`, each name with `[^A-Za-z0-9_-]` replaced by `_`; kind `other` (`tools.js:335-340`). Only a configured prefix says where the server ends: Nessa's server names hold no `__` but may end in `_` | `rawInput.{server, tool}` exactly; title `mcp.<server>.<tool>`, kind `execute`, `_meta.is_mcp_tool_call` (`index.js:23035-23045`) | title `<server>_<tool>` after the same replacement, kind `other`, no `_meta`: cannot be split |
| The tool's `_meta` (`ui.resourceUri`) | not passed on; neither the adapter nor the CLI mentions `resourceUri` | codex has `mcpAppResourceUri` on the call; codex-acp does not copy it | not passed on |
| The result's `_meta` | only in an opt-in `_claude/sdkMessage` notification, not a `session/update` | `rawOutput.result._meta` | dropped |
| `structuredContent` | replaces the result's text blocks as JSON text: indistinguishable from text | `rawOutput.result.structuredContent` | JSON text only when there is no other content |
| resource, resource_link | turned into text by the CLI; never an ACP `resource` or `resource_link` block | only inside `rawOutput.result.content`; no ACP content at all for an MCP call | resource text becomes text; links dropped |
| `rawOutput` | the Anthropic `tool_result` content | `{result: {content, structuredContent, _meta} \| null, error: {message} \| null}` (`index.js:23151-23159`) | `{output, metadata?, attachments?}` |

Unverified: whether codex keeps an MCP result's blocks verbatim in
`rawOutput.result.content` (codex's source was not available, only its strings);
what a Claude PostToolUse hook's `tool_response` holds for an MCP tool;
Opencode's readable source.

What #347 builds on it: the SDK carries an MCP call's server and tool
(`McpTool`) from Claude, split at the one configured prefix that fits (its tool
name in the harness's replaced spelling), and from Codex's `rawInput` exactly,
and names none for Opencode; it carries a Codex MCP result
— its text blocks, and its `structuredContent` bounded as a structured result —
where before a finished Codex MCP call showed nothing. Only Codex's announcement
carries the MCP marker; its completion, which carries the result, does not
(`index.js:25123-25130`), so the adapter remembers the marking per call. Both reach the window on
`ConversationTool` (`mcp`, `structuredContent`).

What it does not: **no harness passes the tool's UI resource through ACP**, so a
tool's `_meta.ui.resourceUri` comes from the server itself, over the gateway's
own connection (#346) — the connection this record already decides the gateway
owns. Until then no call is known to have UI, and the desktop's widget part
(`toolWidget` in `workspace/model/transcript.ts`) is never produced; a call's
text result is shown as before.
