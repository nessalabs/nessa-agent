# Test MCP server

A small stdio MCP server for testing what Nessa receives from MCP tool calls,
and a live check that runs it under a real gateway and a real agent. It is
developer tooling: nothing in the product starts it, and it has no
dependencies beyond Node.

## The server

```sh
node scripts/mcp-test-server/server.mjs
```

It speaks MCP `2025-06-18` over newline-delimited JSON-RPC on stdin and stdout,
answers `initialize`, `ping`, `tools/list`, `tools/call`, `resources/list` and
`resources/read`, and has no side effects. Every answer is fixed, so one
recording can be compared with another.

| Tool | What it returns | What it exercises |
| --- | --- | --- |
| `report_rows` | a text block and `structuredContent` (with an `outputSchema`) | a structured result beside its text |
| `link_resources` | text, a `resource_link`, and an embedded text `resource` | resource blocks |
| `rows.get` | `{"id": 1 \| 2}` → text and `structuredContent`; any other id → `isError` | a dotted tool name, which some harnesses rewrite |
| `always_fails` | a text block with `isError: true` | a tool error result |
| `show_chart` | text and `structuredContent`; the tool declares `_meta.ui.resourceUri` | an MCP Apps tool: `ui://nessa-test/chart.html` is served by `resources/read` as `text/html;profile=mcp-app` |
| `review_rows` | text and `structuredContent`; declares `_meta.ui.resourceUri` `ui://nessa-test/review.html` | an MCP App that calls tools itself: shown inline, once it has its tool result it calls `app_delete_row` and both `model_only_*` tools through the host's `tools/call`, and shows each answer; a button calls `app_delete_row` again, another asks for fullscreen |
| `app_delete_row` | `{"id": 1 \| 2}` → "Deleted row N." (it deletes nothing); any other id → `isError` | a tool only an app may call (`_meta.ui.visibility: ["app"]`), destructive (`destructiveHint: true`), so a host asks the person first |
| `model_only_note` | a text block | a tool hidden from apps (`visibility: ["model"]`) that declares no UI: `resourceUri` is optional in `_meta.ui` |
| `model_only_chart` | a text block | a tool hidden from apps that declares a UI (the chart's) |

The review app's own calls are what the desktop's real-gateway check
(`verification/desktop/scripts/mcp-apps-gateway.mjs`) reads in the window.

Arguments outside a tool's schema return an `isError` result and are never
echoed back. The server's tests are `server.test.mjs`, run by
`pnpm scripts:test`.

To use it from an agent, configure it as a stdio MCP server, for example in a
gateway's `config.json`:

```json
{ "agents": { "mcpServers": [{ "name": "mcptest", "command": "/absolute/path/to/node",
  "args": ["/absolute/path/to/scripts/mcp-test-server/server.mjs"] }] } }
```

## The live check

`live-check.mjs` starts a real gateway in a temporary `ci` namespace, with one
agent whose harness is wrapped by `acp-recorder.mjs` and the test server
configured as `mcptest` (also wrapped, so its own traffic is recorded). The
harness is given a stand-in, `nessa mcp-relay`, in its place, and for each
harness session that starts it the gateway starts the server and holds the
connection to it (ADR 344). It
sends one message asking for every tool once, allows each tool's permission
request once, and writes what happened:

```sh
cargo build -p nessa-server
pnpm exec tsx scripts/mcp-test-server/live-check.mjs claude <out-dir>
pnpm exec tsx scripts/mcp-test-server/live-check.mjs codex <out-dir>
MCP_LIVE_OPENCODE=/path/to/opencode-1.18.31 \
  pnpm exec tsx scripts/mcp-test-server/live-check.mjs opencode <out-dir>
```

| File | Contents |
| --- | --- |
| `acp.jsonl` | every ACP frame between the gateway and the harness, both directions |
| `mcp.jsonl` | every MCP frame between the gateway and the test server, the harness's forwarded calls among them |
| `view.json` | the conversation view the window reads at the end of the turn |
| `summary.json` | the tool frames' shapes, the view's MCP tools with their `resourceUri`, every `ui://` the harness sent, the MCP calls made, the servers the harness was given (stand-ins), and the widget parts the desktop transcript makes of the view |
| `gateway.log` | the gateway's own log |

It uses the sign-in each agent already has on this machine — Claude's
credential from the keychain the gateway reads, Codex's own home, OpenCode's
from Nessa's credential store — and creates none. A gateway that has no
credential for an agent refuses the conversation, and the check stops there.
It allows only calls to the test server's tools, each once, never a standing
approval; anything else the agent asks for is left unanswered. It exits
non-zero unless the turn completed and `show_chart` yielded a widget part,
and removes the gateway's own data
directory (its owner token among it) at the end. Recordings and
`gateway.log` hold the prompt, tool arguments and results, and Codex's
`_auth/status_update` names the signed-in account (its email): review them,
and check in only extracted frames, never a whole recording. The frames the SDK's
parser tests replay (`crates/nessa-sdk/tests/infrastructure/{claude_acp,codex_acp}/tools/fixtures/mcp_live_frames.json`)
were extracted from such a run.

`MCP_LIVE_HARNESSES` names the directory holding `claude-acp/` and `codex-acp/`
with their `node_modules` (default: `crates/nessa-sdk/harnesses` in this
checkout); `MCP_LIVE_NESSA` the gateway binary (default: this checkout's
`target/debug/nessa`); `MCP_LIVE_PORT` (default 7431) and `MCP_LIVE_POLLS` (seconds,
default 300) adjust the run.
