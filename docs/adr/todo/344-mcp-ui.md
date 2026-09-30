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
- Hosts and servers negotiate it as `capabilities.extensions
  ["io.modelcontextprotocol/ui"]`; OpenAI adds optional `openai/*` fields.

Sources: [the MCP extensions spec](https://github.com/openai/mcp-extensions/blob/main/docs/spec.md),
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

**Nessa is an MCP Apps host.** The gateway is an MCP client of the configured
servers for UI: it lists tools with their `_meta.ui`, and reads `ui://`
resources (#346). A tool call's identity, `_meta` and result travel from the
ACP parser to the window's transcript (#347). The gateway offers the window
`mcp.readResource` and `mcp.callTool` for an app, under a policy — only tools
whose `_meta.ui.visibility` includes `"app"`, only on the app's own server,
approval through the existing permission flow for a tool marked
`destructiveHint` — with each call audited (#348). The desktop hosts the app as
an `app`-kind widget (326): a sandbox proxy on a separate origin, a CSP built
only from `_meta.ui.csp` (no network by default), and the `ui/*` bridge mapped
onto the widget host (#349). Nessa declares `io.modelcontextprotocol/ui` with
`text/html;profile=mcp-app`; the `openai/*` fields are optional.

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

- The gateway gains an MCP client and two app methods, with policy and audit;
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
