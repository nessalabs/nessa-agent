# Local Claude ACP binding

This provider guide covers the restricted local Claude harness. Start with the
[agent execution guide](agent_execution/README.md) for domain ownership, lifecycle,
permissions, prompts, and shared transport contracts.

## Composition and application boundary

The host loads its model catalog and supplies a selected `ModelMetadata`, explicit
`TokenLimits`, `infrastructure::acp::sessions::AcpConfig`, and an injected
`Arc<dyn ExecutionAudit>` to `ClaudeAcpProvider::new`. The factory is
immutable. `Agent::new(provider, manager)` opens a new provider context or resumes
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
  WebSearch and WebFetch, plus explicitly configured stdio MCP servers. Native
  file tools retain schema validation; other tools preserve bounded original
  JSON review input. MCP names must belong to a configured server. Native tool
  names are bounded and provider-validated. Existing native review rules and
  configured MCP tools use permission `ask`; only supplied `allow_once` and
  `reject_once` choices are exposed. Ambiguous permission options fail closed.
- Native Bash/BashOutput/KillShell are disabled. Nessa-owned tools, including
  Shepherd-backed shell execution, are exposed through MCP. EnterPlanMode and
  ExitPlanMode are disabled to preserve default permission mode. Form elicitation,
  terminal/filesystem client RPCs, provider-side hooks, plugin/settings-source
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
for these file tests. Authentication/provider failures are surfaced as typed
errors, without provider error text that could contain credentials.

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
