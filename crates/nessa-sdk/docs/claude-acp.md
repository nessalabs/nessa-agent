# Local Claude ACP binding

This provider guide covers the restricted local Claude harness. Start with the
[agent execution guide](agent_execution/README.md) for domain ownership, lifecycle,
permissions, prompts, and shared transport contracts.

## Composition and application boundary

The host loads its model catalog and supplies a selected `ModelMetadata`, explicit
`TokenLimits`, `infrastructure::acp::sessions::AcpConfig`, and an injected
`Arc<dyn ExecutionAudit>` to `ClaudeAcpProvider::new`. The factory is
immutable. `Agent::prepare(provider, manager, audit)` loads durable evidence. A separate attributed attachment opens a new provider context or resumes
the context saved under the manager's local session key. There are no global
backend handles, environment reads in the adapter, or fallback models.

`Agent` owns public invocation, controls, hooks, and automatic snapshot persistence.
`application::agent_execution::providers` owns the adapter ports and its
`OpenedProviderSession` return value: a `ProviderSession` control handle and one
`ExecutionEventStream` reader consumed by Agent. JSON, subprocess handles, and ACP
request IDs stay in infrastructure.

```text
host -> Agent(provider, SessionManager)
          |-> ClaudeAcpProvider -> ProviderSession -> ACP worker
          |-> SessionManager -> SessionStorageLease
          |-> hooks + optional UI subscribers
```

Arrows show construction, calls, and observations. The shared ACP worker owns protocol
bookkeeping and delegates execution invariants to the domain session. Conversation
acceptance, durable turn identity, receipts, and transcript lifecycle remain separate.
The host must authorize prompts and permission answers before invoking this port.
Agent drains the event reader independently of UI subscribers. The host must await
`Agent::close(actor)` before shutting down its async runtime. Dropping every session handle requests cleanup but cannot
provide an awaited cleanup guarantee after the runtime itself has stopped.

Failure-initiated shutdown keeps invocation settlement separate from process cleanup.
For an active invocation, ACP maps `SessionCloseRequest::ExecutionFailed` and
`SessionFailed` to `Err(AgentError::Closed)`, and `DeadlineExceeded` to
`Err(AgentError::Deadline)`, unless an existing operation failure provides the primary
error. Finish and closure audit retain the exact initiating domain reason and active
execution identity. Provider acknowledgement of cancellation cannot replace that
failure with `Ok(Cancelled)`. Audit or cleanup failures remain visible alongside the
invocation failure. Ordinary caller close can settle interrupted work as
`Ok(ExecutionOutcome::Cancelled)`; a successful shutdown result independently confirms
cleanup. An already-settled invocation keeps its earlier result.

## Supported native profile

- Unix process groups; Windows configuration is rejected before starting a child.
- Exact Anthropic model selected from the catalog. Process-scoped
  `ANTHROPIC_MODEL` and `ANTHROPIC_CUSTOM_MODEL_OPTION` plus the session model
  option preserve the exact picker entry. Startup checks the returned model and
  default permission mode. Duplicate model or mode configuration IDs are rejected
  before selecting values, even when repeated values agree. The same validation
  applies to startup/restoration replies and live updates. Later model/mode drift closes the binding. Managed
  restrictions are not rewritten to make a selection succeed.
- Text prompts/output with an optional Claude-native tool preset, including
  WebSearch and WebFetch, plus explicitly configured stdio MCP servers. The
  preset defers tool schemas: the model calls ToolSearch to load a tool before
  it may call that tool at all. Every tool the harness offers — its built-ins
  and the tools of configured MCP servers — is therefore reviewable, and is
  routed to Nessa's permission owner by a single `ask` rule. A built-in this
  adapter has never heard of is reviewed with its input preserved rather than
  refused; refusing one ended the whole execution. An unfamiliar *name* is what
  this covers: denied names still fail closed. A structurally tagged tool-result
  block this adapter does not render becomes a fixed visible placeholder, while
  malformed known text/diff shapes still end the execution. Native file tools
  validate required known fields and their types while preserving additive
  provider fields in the exact review JSON; other tools preserve bounded
  original JSON review input.
  MCP names must belong to a configured server. Native tool names are bounded
  and provider-validated. Only supplied `allow_once` and `reject_once` choices
  are exposed. Ambiguous permission options fail closed.
- Denied tools are the whole of that boundary, since admission is otherwise
  open, and they are read against the one pinned harness version startup
  verifies. Execution Nessa does not own is denied: Bash, TaskOutput, TaskStop,
  Monitor — which takes a shell command or a WebSocket — and REPL. The pinned
  Claude SDK canonicalizes the historical BashOutput and KillShell spellings to
  TaskOutput and TaskStop before permission-rule matching, so the configured
  names state the effective boundary directly. Nessa-owned
  tools, including Shepherd-backed shell execution, are exposed through MCP.
  EnterPlanMode and ExitPlanMode are denied to preserve default permission mode.
  Work that would outlive or escape the execution that asked for it is denied
  too: Workflow, CronCreate/CronDelete/CronList, and EnterWorktree/ExitWorktree.
  So are effects on services beyond this machine: Artifact, PushNotification,
  RemoteTrigger and SendFeedback. Reviewing a tool is not the same as owning
  what it does.
- Form elicitation, terminal/filesystem client RPCs, provider-side hooks, plugin/settings-source
  loading, explicit reasoning controls and extended context remain unsupported.
  Claude omits tools that require unadvertised client capabilities, such as
  AskUserQuestion. Unsupported incoming client requests receive a protocol error.
- Binding ceilings of **200,000 context tokens and 64,000 output tokens**, further
  narrowed by model facts and the host's explicit limits. These are this profile's
  limits, not new metadata or inferred provider defaults. The fixed output limit
  is supplied to the harness through `CLAUDE_CODE_MAX_OUTPUT_TOKENS`; each prompt
  reserves that same output ceiling. Context/token counts are local admission
  inputs supplied by the caller, not measurements of the harness's hidden system
  prompt, history, auto-compaction, or actual provider usage. This slice does not
  offer provider-exact occupancy reporting.

Programmatic session settings disable hooks and Claude.ai connectors and allow
no MCP servers. SDK filesystem settings sources are empty. The upstream adapter
still reads/watches its own settings for picker/policy behavior; returned runtime
configuration is checked, never used to update Nessa's model catalog.

Process-group supervision owns the ACP provider process. Native Claude tools are
enabled through `tools_enabled`; native shell execution is disabled in favor of
trusted MCP tools. This is not containment for a malicious harness or arbitrary
commands that deliberately detach. Nessa's MCP shell separately owns command
scopes through Shepherd and retains their process/audit results. See the
[MCP server guide](../../nessa-mcp/README.md) for platform and retention limits.

## Queueing, steering, and hooks

Agent owns pending-input admission, priority ordering, and withdrawal. Normal ACP
prompts dispatch one at a time, preserving execution/output correlation. Native
steering uses the initialized profile’s advertised `_session/steering` extension;
acknowledged input contributes to the active execution. Only an explicit
`PromptRequired` response permits a new queued invocation. Ambiguous delivery is
never retried as a prompt, and the control response has a five-second deadline.

Before either prompt form writes to the provider, the shared ACP transport drains
decoded input and establishes an operating-system pipe boundary. Unix consumes any
bytes already available through a nonblocking read; Windows checks the pipe and
awaits the registered asynchronous read when bytes are present. Policy changes,
provider closure, and incomplete frames observed before that boundary therefore
settle before dispatch. Input concurrent with or later than the empty-pipe boundary
belongs to the next transport turn.

The SDK’s before/after invocation hooks run locally around dispatch. They are
distinct from provider-side hooks disabled by this restricted profile. See the
[scheduling contract](agent_execution/scheduling.md) for APIs, removal, failure
evidence, and deterministic regression tests. These tests do not establish live
provider compatibility beyond the dated checks below.

## Deleting a session

`ClaudeAcpProvider` implements `ProviderSessionDeleter`. The adapter advertises
`sessionCapabilities.delete`; on `session/delete` it tears down any live query
for the session and deletes the Claude Code session file, so a successful answer
is reported as `ProviderSessionDeletion::Deleted`. The exchange opens its own
connection (initialize, then the delete) and never resumes the session. The
adapter also advertises `sessionCapabilities.list`, for the workspace, and sends
its whole list in one frame. It leaves out a session with no titled prompt, so a
conversation of images alone is not listed; the delete is sent regardless, and
deletes it. Its delete of a session it no longer has is an error: when the list,
read after that error, does not name the session, it settles as
`ProviderSessionDeletion::NotListed`, which is how a deletion interrupted after
Claude deleted finishes.

## Install and run

The local harness manifest and lockfile pin
`@agentclientprotocol/claude-agent-acp` **0.76.0**, ACP JavaScript SDK **1.4.0**,
and Claude Agent SDK **0.3.257**. Runtime initialization requires protocol **1** and
adapter **0.76.0**. Install from the repository root:

```sh
npm ci --prefix crates/nessa-sdk/harnesses/claude-acp --ignore-scripts --no-audit --no-fund
```

[The Rust example](../examples/claude_acp.rs) performs explicit composition using
normal local Claude authentication. It does not extract or copy credentials. It
uses a 100,000-token admission window and a 1,000-token output reservation, and no
prompt timeout. `TokenLimits::new(context_window, max_output)` validates those
positive budgets and requires output to fit within context. In this example,
input plus 1,000 reserved output tokens must fit within 100,000. The output ceiling
is per model response, not a lifetime token budget for the agent; the context
value is local admission configuration, not a measurement or an instruction to
resize the harness's context. These small values are smoke-test settings, not
production defaults. Both examples emit structured `tracing` events, with the
subscriber installed by example composition:


```sh
cargo run -p nessa-sdk --example claude_acp -- \
  crates/nessa-sdk/data/models.json \
  /absolute/path/to/node \
  /absolute/repo/crates/nessa-sdk/harnesses/claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js \
  /absolute/existing/workspace \
  claude-haiku-4-5-20251001 100 'Reply with hello.' text
```

Supply the prompt's input count explicitly; the example does not tokenize it.
Other smoke modes are `close` (after first text), `deny-write`, `close-write`
(while permission is pending), and `verify-write`. The last mode allows only one
Write to `nessa-binding-smoke.txt` in the selected workspace with exact content
`Nessa binding smoke test\n`; every other request is denied. Use a scratch workspace
for these file tests. Authentication/provider failures retain their numeric code
as the typed decision fact and, when supplied, at most 4 KiB of provider text as
diagnostic context. The diagnostic does not decide admission, settlement, or
cleanup.

## Verification

The shared-runtime extraction passes the SDK and shared backend regression tests,
Clippy with warnings denied, and the declared Rust 1.89 build. The domain gate
reports 100% line, function, and region coverage with no exclusions added.


On **2026-09-12, macOS**, the real Rust adapter with the pinned harness and exact
`claude-haiku-4-5-20251001` model completed these checks using existing local auth:

| Check | Observed result |
| --- | --- |
| Text prompt | Expected text; `Completed`; cleanup without force |
| One exact Write permission | One approval; exact file content verified; `Completed` |
| Denied Write | Denial observed; requested file absent |
| Close after first streamed text | `Cancelled` after cleanup without force |
| Close with pending Write permission | `Cancelled`; requested file absent |

The initial live-check baseline passed 57 SDK tests, Clippy, formatting, and the
declared Rust 1.89 build. Current session, transport, and profile-substitution
regressions are covered by the SDK test suite and the full domain coverage gate;
this extraction adds no domain exclusions. The file/cancellation checks above describe the earlier baseline.
After this simplification, the live example configured a custom system prompt
naming the assistant Nessa, asked for its name, and received `Nessa`, `Completed`,
and `CloseOutcome { forced: false }` with no tool approvals. The deterministic
fixture separately verifies that instructions appear only in session creation and
two successive executions send only their respective new user message.

On **2026-09-20, macOS**, the pinned harness was driven directly over stdio to
record what it advertises and what it does with each kind of prompt content.
Everything below is pinned to adapter **0.76.0** and must be re-verified when that
pin moves.

`initialize` returns `promptCapabilities` with exactly two keys:

```json
{
  "agentCapabilities": {
    "promptCapabilities": { "image": true, "embeddedContext": true }
  }
}
```

This adapter advertises no `audio` capability, though the protocol defines one:
the difference is the adapter's, not ACP's. The object is a literal in the adapter, not
something derived from the model, the account, or the client's advertised
capabilities (`dist/acp-agent.js:916-919`). The SDK reads only `image`
(`src/infrastructure/acp/executions/worker.rs:619`); `embeddedContext` is read
nowhere in this repository.

ACP defines five content kinds and no more: `text`, `resource_link`, `image`,
`audio`, and `resource` carrying either text or a blob. `text` and `resource_link`
are baseline and every agent must accept them; the rest are opted in to through
`promptCapabilities`. There is no document kind and no video kind. What this
adapter does with each, from `promptToClaude` (`dist/acp-agent.js:7002-7075`):

| Prompt block | What reaches the model |
| --- | --- |
| `text` | The same text |
| `resource_link` | A text block holding `[@name](uri)`; the file is not opened |
| `resource` carrying `text` | That link, then the text inlined as `<context ref="uri">` after all other content |
| `resource` carrying `blob` | Nothing; discarded at `dist/acp-agent.js:7038` |
| `audio` | Nothing; discarded at `dist/acp-agent.js:7062` |
| `image` with `data` | A base64 image block |
| `image` with an `http` `uri` and no `data` | A URL image block the provider fetches |

Discarding is silent. A prompt carrying a blob resource and an audio block beside
its text returned `stopReason: "end_turn"` with an ordinary usage record and no
error, and the model's own account of what it received named neither. A client
that sends content this adapter does not carry gets no signal that part of its
message vanished, so a binding must refuse such content locally rather than send
it hopefully.

A `resource_link` is text, not an instruction to open anything. The model may then
choose to call its own `Read` tool on the path, which is an ordinary tool call:
under this profile every tool the harness offers is in permission `ask`
(`REVIEWED_TOOLS_RULE` in `src/infrastructure/claude_acp/tools/wire.rs`), so
every such read raises
`session/request_permission`, including a path inside the launch workspace that
the adapter's own default would have allowed without asking. The workspace is the
child's working directory, not a sandbox: an approved read outside it succeeds. A
linked PDF was read this way and summarized correctly, so a document reaches the
model through the agent's file tool and a user approval, never through a prompt
payload.

This is what `LinkedFile` and its `resource_link` block are built on: see
[prompts](agent_execution/prompts.md) for the value object and
[ADR 0013](../../../docs/adr/done/0013-files-by-path-not-by-payload.md) for why a
file that is not an image is named rather than carried. Two details of that
binding follow from the table above and are worth stating here. The adapter
writes `[@name](uri)` as plain text, so the `uri` and the `name` are both text
the model reads and both are interpolated into markdown by code this repository
does not own. Neither is allowed to be syntax, and that is enforced by deciding
what may appear in each rather than by listing what may not: the URI is
percent-encoded down to the unreserved set plus `/` and `%`, and the label
backslash-escapes every ASCII punctuation character, which is exactly the set
CommonMark defines an escape for. A file named `report (final).pdf` therefore
arrives as `[@report \(final\)\.pdf](file:///Users/ada/report%20%28final%29.pdf)`
— noisier to read than it was, and immune to a grammar nobody here has to have
understood correctly. Three successive attempts to name the dangerous characters
instead — brackets, then `)`, then `\` — were each wrong, which is the argument
for the allowlist and is recorded in `prompt_link_attacks.rs`. And
`embeddedContext` remains read nowhere: the `resource`-with-text route is
deliberately not taken, because it would mean transporting the file.

**Unverified against a live model, and worth re-checking when the pin moves:**
whether a model reliably percent-decodes a URI such as
`file:///Users/ada/my%20report.pdf` before passing a path to `Read`. The PDF case
above did not exercise a path needing encoding.

These facts rest on two different strengths of evidence and the difference is the
useful part. The capability literal and the conversions and discards in the table
above were read from the adapter's source, which establishes that a discard is silent
rather than merely unobserved on one run. The `initialize` response, the
end-to-end fate of every block kind, the permission request per `Read`, and the
PDF case were observed in live runs against `claude-haiku-4-5-20251001` using
existing local auth. No SDK test establishes any of it: the
`claude_acp_test_handler.py` fixture below advertises only `{"image": …}`, so it is
evidence about this SDK's own handling and never about what the real Claude agent
accepts.

The Python ACP test handlers test protocol faults, startup deadlines, permission
correlation, sparse patches, queue overflow, repeated Close, completion races,
session isolation, dropped handles, and a TERM-resistant parent with a reaped
child. They are adapter/process tests, not provider compatibility evidence.
Both handlers live under `tests/infrastructure/acp/contracts/fixtures/`: `claude_acp_test_handler.py` is
launched by the integration tests, and `test_acp_handler.py` is launched by the
shared transport's `TestAcpProfile` in its `#[cfg(all(test, unix))]` test module.
Neither handler is registered in application composition or selected by a runtime
environment variable. Running the SDK tests requires no model credentials or calls.
The app already has `dev`, `ci`, `alpha`, and `prod` stages; its conversation
scenarios require a development build in `dev` or `ci`. Those UI scenarios do not
select an ACP test handler. See [dependency injection](../../../docs/design/dependency-injection.md).
Application tests substitute independent session implementations without ACP.
The existing CI job runs SDK tests and Clippy on macOS/Linux/Windows; native
process tests run on Unix and need `/usr/bin/python3`. Live authenticated checks
are manual. Linux live-provider behavior, Windows process ownership, unrestricted
detached execution, crash recovery, and durable Conversation semantics are not
claimed by these macOS results.

Sources for the pinned behavior: [Claude ACP source](https://github.com/agentclientprotocol/claude-agent-acp/tree/c2e4815),
[custom model options](https://code.claude.com/docs/en/model-config#add-a-custom-model-option),
and [ACP prompt cancellation](https://agentclientprotocol.com/protocol/prompt-turn#cancellation).

`AcpConfig.environment` holds noncredential context selectors such as `HOME`,
`CLAUDE_CONFIG_DIR`, endpoint URLs, and search paths. `credential_environment`
contains only host-supplied credential values. Both maps are passed verbatim to the
child after clearing inherited variables, and their keys must be disjoint. The
adapter does not discover or extract credentials. Never place credentials in
arguments or the context environment.

Restoration identity is a SHA-256 fingerprint of the executable, ordered arguments,
context environment, workspace, tool policy, model limits, and composed system
prompt; model identity is recorded separately. Credential values are excluded and
never stored in session snapshots. Credential rotation assumes the same intended
provider account and context. Hosts switching accounts must keep the account or
profile namespace in noncredential context configuration; excluding secrets does
not verify account identity. Context changes reject old snapshots before launching
or resuming the provider.
