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

`--initialize-delay-ms <ms>` holds its answer to `initialize` that long, every
answer still in order, so a check can see a host while the server is starting
(`mcp-servers-gateway.mjs`'s `inspect` step).

| Tool | What it returns | What it exercises |
| --- | --- | --- |
| `report_rows` | a text block and `structuredContent` (with an `outputSchema`) | a structured result beside its text |
| `link_resources` | text, a `resource_link`, and an embedded text `resource` | resource blocks |
| `rows.get` | `{"id": 1 \| 2}` → text and `structuredContent`; any other id → `isError` | a dotted tool name, which some harnesses rewrite |
| `always_fails` | a text block with `isError: true` | a tool error result |
| `show_chart` | text and `structuredContent`; the tool declares `_meta.ui.resourceUri` | an MCP App: `ui://nessa-test/chart.html` is served by `resources/read` as `text/html;profile=mcp-app`. The page handshakes (`ui/initialize`, then `initialized` and `size-changed`), draws `structuredContent.series` from `ui/notifications/tool-result` into `#chart` (`alpha 10, beta 20`), and answers `ui/resource-teardown` with `{ result: {} }`. `verification/desktop/scripts/mcp-apps.mjs --only chart` shows that page go live |
| `review_rows` | text and `structuredContent`; declares `_meta.ui.resourceUri` `ui://nessa-test/review.html` | an MCP App that calls tools itself: shown inline, once it has its tool result it calls `app_delete_row` and both `model_only_*` tools through the host's `tools/call`, and shows each answer; a button calls `app_delete_row` again, another asks for fullscreen, and two more send `ui/message` and `ui/update-model-context` when clicked |
| `app_delete_row` | `{"id": 1 \| 2}` → "Deleted row N." (it deletes nothing); any other id → `isError` | a tool only an app may call (`_meta.ui.visibility: ["app"]`), destructive (`destructiveHint: true`), so a host asks the person first |
| `model_only_note` | a text block | a tool hidden from apps (`visibility: ["model"]`) that declares no UI: `resourceUri` is optional in `_meta.ui` |
| `model_only_chart` | a text block | a tool hidden from apps that declares a UI (the chart's) |

The review app's own calls are what the desktop's real-gateway check
(`verification/desktop/scripts/mcp-apps-gateway.mjs`, on
`verification/desktop/scripts/lib/gateway-stack.mjs`) reads in the window.

Arguments outside a tool's schema return an `isError` result and are never
echoed back. The server's tests are `server.test.mjs`, and the local
gateway's are `local-gateway.test.mjs`, both run by `pnpm scripts:test`.

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
sends one message asking for five of the server's tools, each once, and writes
what happened. The message is `toolPrompt` in `local-gateway.mjs`, which the
desktop's `mcp-apps-gateway.mjs` asks with too: it lets the agent use its own
tool search, since Codex reaches MCP tools only through it (#500), and forbids
only other tools of the server.

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
| `gateway.log` | the gateway's own log, read once it has stopped (or, if it failed to start, what it said then) |

It uses the sign-in each agent already has on this machine — Claude's
credential from the keychain the gateway reads, Codex's own home, OpenCode's
from Nessa's credential store — and creates none. A gateway that has no
credential for an agent refuses the conversation, and the check stops there.
Which permission requests it answers, and what it checks, is stated once, in
`live-check.mjs`'s header. It exits
non-zero unless the turn completed and `show_chart` yielded a widget part,
and removes the gateway's own data
directory (its owner token among it) at the end. Recordings and
`gateway.log` hold the prompt, tool arguments and results, and Codex's
`_auth/status_update` names the signed-in account (its email): review them,
and check in only extracted frames, never a whole recording. The frames the SDK's
parser tests replay (`crates/nessa-sdk/tests/infrastructure/{claude_acp,codex_acp}/tools/fixtures/mcp_live_frames.json`)
were extracted from such a run. The Codex fixture's `toolSearchTurn` is one
turn asked with `toolPrompt`'s single-tool wording, which
`local-gateway.test.mjs` holds to it: change the wording, and record it again.
No checked-in command sends that wording under the recorder: it was recorded
with this check's setup (`startLocalGateway`, the harness wrapped by
`acp-recorder.mjs`) sending `toolPrompt([{ name: "review_rows" }])`, and
extracted as the `session/prompt` text and every `tool_call` and
`tool_call_update` frame, verbatim. Codex asked no permission for it.

`MCP_LIVE_HARNESSES` names the directory holding `claude-acp/` and `codex-acp/`
with their `node_modules` (default: `crates/nessa-sdk/harnesses` in this
checkout); `MCP_LIVE_NESSA` the gateway binary (default: this checkout's
`target/debug/nessa`); `MCP_LIVE_PORT` (default 7431) and `MCP_LIVE_POLLS` (seconds,
default 300) adjust the run.

## The scripted agent

`scripted-agent.mjs codex|claude <tool>` is a stdio ACP agent with no model,
which a gateway can run as that agent's runtime (an explicit `command`). It
answers the handshake as the harness pinned in
`crates/nessa-sdk/harnesses/<agent>-acp/package.json`. A session's first
completed prompt makes one real call of `<tool>`, with the recorded call's
arguments, through the stand-in the gateway gave it for `mcptest`. As Claude, that call carries the call's id in
`_meta["claudecode/toolUseId"]`, where Claude's harness names a forwarded call
and the gateway's stand-in keeps its `structuredContent` for that call (the
SDK's `CALL_ID` in `stand_in.rs`, which a test holds `scripted-frames.mjs`'s
`CLAUDE_CALL_ID` to); as Codex, it names no call id (no
`_meta["claudecode/toolUseId"]`), as Codex's harness names none. This is the
one value taken from the harness's MCP side rather than from the recordings,
which hold only ACP frames. It then reports
that call in the frames the harness was recorded sending, under the same id,
says DONE, and ends the turn. A later prompt, after that turn ended, answers
with the text "Noted." and calls nothing, so a message an app sends has an idle
turn to land in. A prompt after a cancelled first turn is still the recorded
call: the session marks that call only after a turn that was not cancelled, so
it has not completed one. Claude, and a Codex call other than the one
Codex ran without asking, are the recorded `show_chart` call from the parser
fixtures above, value for value, with only the call's id, its tool's name and
the server's result written at the places that harness carries them
(`scripted-frames.mjs`'s `PLACES`). Codex ran `review_rows` without asking, and
a replay of that tool is the fixture's `toolSearchTurn`: `tool_call`, then
`tool_call_update` `completed`, with no bare `in_progress` update. The test
checks the places against the `show_chart` recordings, and holds the unasked
replay to `toolSearchTurn`. A recording that carries the call anywhere else
fails the places check. It replays only what that call can stand for: one of
the test server's tools, under a name no harness rewrites, with the recorded
call's arguments, whose result is shaped as the recorded one is (the same
keys, as many text blocks, a non-empty object of `structuredContent`). Anything else — a
failure, a text-only result, a dotted name — is refused, since the harnesses
report those in frames of their own. A cancel during the call ends the turn
`cancelled` with nothing reported, whether the call then answers or fails. It does not ask permission for the call, as a harness
does: the recordings hold no permission request.

It reads no credential: `startLocalGateway({ signedOut: true })` starts the
gateway from `PATH`, `TMPDIR` and `RUST_LOG` alone, with a home of its own and
a placeholder `ANTHROPIC_API_KEY` (which keeps the gateway from reading
Claude's from the keychain). The desktop's real-gateway check runs it with
`--scripted`. Its design table is on #418, and its tests are
`scripted-frames.test.mjs` and `scripted-agent.test.mjs`.

`scripted-agent.mjs codex|claude --scenario <file>` runs that file's steps
instead of the recorded frames (`scripted-scenario.mjs`). A step emits text,
asks a permission and follows the answer (allow once, deny once, or a
withdrawn review), calls an MCP tool, fails the turn, waits for
`session/cancel`, or ends it. `--scenario` replaces that path for every prompt
the file answers. A run without `--scenario` is the default above: the first
completed prompt is one recorded call, and a later prompt is text and no tool
call. `scenarios/text-reply.json` is the
plain reply `gateway-window.mjs --scripted` uses, and the turn that calls
`review_rows` when the prompt says "show the server's app". `scenarios/window.json` is
the permission, failure and cancel `scripted-scenarios.mjs` drives. The
state table is in `scripted-scenario.mjs`, and its tests are
`scripted-scenario.test.mjs`.

`pnpm test:e2e:scripted` builds the gateway and runs the signed-out checks in
Chromium and WebKit. It does not need harness `node_modules`. The summary it
writes is what a pull request that changes UI, gateway, ACP, or MCP behavior
shows ([Browser verification for UI](../../CODING_STANDARDS.md#browser-verification-for-ui)).

## HTTP contract fixture

`http-server.mjs` recovers the developer HTTP fixture from commit `b93383c3d`
(branch `392-remote-mcp`), imports current `server.mjs` answers and preserves
its stdio behavior. No product HTTP client, remote configuration or OAuth is
implemented by this fixture.

```sh
node scripts/mcp-test-server/http-server.mjs 8931
node scripts/mcp-test-server/http-server.mjs 8931 --sse
node --test scripts/mcp-test-server/http-server.test.mjs
node scripts/mcp-test-server/capture-http.mjs /tmp/http-frames.json
```

The tests need only bare Node and loopback networking; no credentials, harness
installation or live model. Each network read has a five-second client budget.
The fixture listens only on `127.0.0.1`. Streamable HTTP serves `/mcp` with JSON
responses; initialize creates an `Mcp-Session-Id`, subsequent requests need it,
notifications return 202, GET returns 405 and DELETE removes the named session.
Legacy HTTP+SSE returns 405 for POST `/mcp` and an endpoint event on GET; requests
POSTed to that endpoint return 202, with replies as message events on that stream.
Closing one stream removes that endpoint while another stream continues.

| Module | Responsibility |
| --- | --- |
| `http-server.mjs` | Developer transport, loopback listener, session/stream ownership, request bounds |
| `capture-http.mjs` | Credential-free refresh of the local HTTP corpus with source hashes |
| `fixtures/http-frames.json` | Recorded local HTTP initialize, ping, structured/chart and error results, UI resource and invalid tool response |
| `http-server.test.mjs` | Replay the corpus through JSON and SSE; check session isolation, challenges, malformed bodies, expiry and cleanup |

The corpus records actual loopback HTTP responses, with session headers omitted;
it is local deterministic MCP test-server evidence, not live remote provider data.
Tests compare the response payloads with those recorded frames, including app
metadata/resource contents. Session tests use the actual HTTP headers/endpoints
from each live local listener.

| State/event | Fixture outcome and test evidence |
| --- | --- |
| Initialize | New session; headers distinguish two independently opened sessions |
| Missing/foreign/deleted identity | 400 for missing, 404 for unknown; survivor still answers after another session's DELETE |
| Unauthorized | Optional `serveHttp({ bearerToken })` challenges with 401/`WWW-Authenticate: Bearer` before creation/deletion; authorized session survives rejected deletion |
| Expiry | Absolute TTL checked on incoming requests; injected `now` tests exact expiry without sleeping; an expired legacy stream ends |
| Legacy disconnect | Endpoint removed when its owning stream closes; bounded polling verifies 404 and a second stream survives |
| Malformed/oversized body | 400 parse/invalid request or 413 beyond 1 MiB; next valid session still initializes and answers |

`bearerToken` is a developer fixture option, not OAuth or a stored credential.
The request body retains at most 1 MiB and drains the remainder before replying;
there is no independent per-request server deadline. Expiry is lazy: a request
checks/removes expired sessions, so an idle legacy stream does not close just
because time passes. These are fixture limits, not promises for the product
transport. No test here proves Nessa session isolation, authenticated refresh,
remote network cancellation, restart recovery or physical provider cleanup; those
remain the implementation slices in [ADR 392](../../docs/adr/todo/392-remote-mcp-servers.md).

The HTTP replay suite also recomputes `server.mjs`, `http-server.mjs` and
`capture-http.mjs` SHA-256 values against the corpus provenance. Changing any
producer without refreshing its corpus fails the check even if current replies
still match. `.gitattributes` keeps those hashed sources at LF on each checkout.
