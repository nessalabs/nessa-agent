# Local Claude ACP binding

The SDK can now open a local Claude session, stream typed text and file-tool
observations, resolve once-only permissions, and Stop its owned process scope.
This is a reusable execution adapter. The Conversation aggregate/coordinator,
durable acceptance and replay, gateway RPCs, and UI wiring remain separate work.

## Composition and application boundary

The host loads its model catalog and supplies a selected `ModelMetadata`, explicit
`TokenLimits`, and `ClaudeAcpConfig` to `ClaudeAcpBinding::new`. The factory is
immutable; each `AgentBinding::open` starts an independent process group and ACP
session. There are no global backend handles, environment reads in the adapter,
model aliases, automatic retries, or fallback models.

`application::agent_binding` owns the object-safe `AgentBinding`, `AgentSession`,
and `AgentTurnEvents` ports. `OpenedBinding` returns the session, one event reader,
and the effective capability snapshot created for that exact configuration.
`Agent` coordinates capability admission through domain validation before calling
the injected session. The Claude port also validates direct callers. Provider
JSON, subprocess handles, and ACP request IDs stay in infrastructure.

```text
host composition --> selected model + config --> ClaudeAcpBinding
                                                   |
                                              open one scope
                                                   |
caller --> Agent --> AgentSession <-------------- worker
                       |                           |
                 permission / Stop           bounded ACP stdio
caller <----------- AgentTurnEvents <---------------+
```

Arrows show construction, calls, and observations. The worker owns protocol
bookkeeping, not Conversation acceptance, turn identity, receipts, or lifecycle.
The host must authorize prompts and permission answers before invoking this port.
It must keep the event reader draining, and await `stop()` before shutting down
its async runtime. Dropping every session handle requests cleanup but cannot
provide an awaited cleanup guarantee after the runtime itself has stopped.

## Reusable execution domain

`domain::agent_execution` owns provider-independent concepts, grouped by DDD role:

- `value_objects`: immutable `Prompt`, validated `PromptText`, execution/tool/permission
  identities, message fragments, file paths and locations, tool kinds/status/content,
  `ToolCallUpdate`, `FileToolInput`, outcomes, and permission configuration/choices.
- `builders::PromptBuilder`: composes text from supplied sources in insertion order
  and validates the completed prompt. It preserves all text and adds no implicit
  separators. Source loading and tool-schema serialization remain outside the domain;
  typed tool-schema content can be added when that contract is implemented.
- `entities::ToolCall`: holds one execution's observed tool state. `apply` rejects
  updates for another execution or tool, preserves omitted fields, and replaces
  collections explicitly supplied as empty. It never runs a tool.
- `entities::PermissionRequest`: binds the request, execution, tool, input, and
  configured offered options. Answers must select an offered option ID in the
  correct execution. A request can resolve only once; invalid answers leave it
  pending. `cancel` closes a pending request. Host authorization is separate.
- `events::AgentTurnEvent` and `AgentTurnUpdate`: immutable execution-correlated
  observations for messages, tools, permissions, and completion. They contain no
  provider/transport errors or persistence machinery. `AgentTurnEvents` is only
  the application reader port; delivery failures use its error result.

`Prompt` holds reusable content; `PromptRequest` carries it with execution ID and
caller-supplied token admission counts. Application mapping validates execution
metadata. Blank prompt content cannot reach the port because domain construction
already rejects it. For example:

```rust
use nessa_sdk::domain::agent_execution::builders::PromptBuilder;

let prompt = PromptBuilder::new()
    .text("Review the proposed change.")
    .text("\n\n")
    .text("Focus on permission handling.")
    .build()?;
```

`PermissionDecision` represents allow/reject once and allow/reject always.
`PermissionConfig` controls which kinds can be offered; it grants nothing by
itself. `PermissionOptions` validates option identities and filters choices through
that configuration. Multiple choices can have the same kind but different scopes,
so `PermissionAnswer` selects an exact `option_id`, not an allow/deny boolean.
The request records that option ID with its decision. Domain resolution records
intent; adapters or authorization services apply the effect.

The current Claude profile requires `PermissionConfig::once_only()` or a subset
of its decision kinds. It rejects persistent configuration before spawning: the
pinned adapter's persistent options can write durable rules or change permission
modes, and their structured scopes are not exposed by this binding yet. Other
adapters can reuse the full domain model. No persistent grant is silently treated
as a once-only grant.

The worker keeps RPC IDs and process effects outside the domain and uses domain
entities for observation merging and permission resolution. Permission events
include the tool state merged so far and the exact configured options. Normal
tool events remain sparse. Paths are untrusted descriptions, never filesystem
access or authorization; wire size bounds remain adapter rules.

`MessageChunk` models streamed text or thought fragments, including empty and
whitespace-only fragments. A complete conversation message entity still needs
transcript identity and lifecycle, which this binding does not own.

## Supported native profile

- Unix process groups; Windows configuration is rejected before starting a child.
- Exact Anthropic model selected from the catalog. Process-scoped
  `ANTHROPIC_MODEL` and `ANTHROPIC_CUSTOM_MODEL_OPTION` plus the session model
  option preserve the exact picker entry. Startup checks the returned model and
  default permission mode. Later model/mode drift closes the binding. Managed
  restrictions are not rewritten to make a selection succeed.
- Text prompts/output; optional built-in `Read`, `Write`, `Edit`, `Glob`, and
  `Grep` tools. Every enabled tool is put in the permission `ask` list. Only
  supplied `allow_once` and `reject_once` choices are exposed. Unknown tools,
  ambiguous options, and unrecognized permission input fields fail closed.
- No Bash, delegated agents, MCP servers, terminal or filesystem client RPCs,
  hooks, plugin/settings-source loading, explicit reasoning controls, or extended
  context mode. Unsupported incoming client requests receive a protocol error.
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

Process-group supervision is for this restricted file-tool profile. It is not
containment for a malicious harness or arbitrary detached commands. Shell and
agent execution require a stronger host ownership mechanism before they can be
enabled; there is no switch that opts into those unsupported modes here.

## Streaming, permission, and Stop contracts

Each prompt carries a host-owned `execution_id`. Every observation and terminal
`Finished` update carries that ID, so queued updates cannot be attributed to the
next prompt. Permission answers must match execution, request, and offered option IDs.
The host supplies a new identity for each attempt; this port does not implement
request deduplication or durable acceptance. Only one prompt may be active per
session; a second returns `Busy`. Commands and
events use bounded channels. Frames and persistent identifiers have size limits.
`prompt_timeout: None` is the default policy: a prompt has no elapsed-time cutoff.
A host can explicitly opt into a total runtime limit with `Some(duration)`.
Elapsed runtime or quiet output is not a stuck-agent diagnosis. Startup, protocol
writes, and cleanup keep their separate deadlines, and Stop remains available
throughout an unlimited prompt. Health assessment is separate future work.

Output queue overflow is a binding failure; events are not silently dropped while
execution continues. The worker tears down the scope, then the reader reports the
failure after previously queued events. Advisory usage/plan/metadata extensions
are bounded and ignored; they cannot change capabilities or imply success.

Tool events are sparse patches: `None` means omitted, while `Some([])` replaces a
collection with an empty collection. Text and file diffs are normalized into
Nessa-owned content. Permission events contain typed file inputs, including
paths and proposed contents. Those values are untrusted data for the host's
policy/review, never authority by themselves. Answers use local, session-scoped
IDs; already-answered or stale IDs cannot approve a later request.

Prompt results distinguish completion, output limit, request limit, refusal, and
cancellation. An unknown stop reason is a protocol failure. Drain the ordered
`Finished` update as well as awaiting the prompt result: a result future can be
ready before its preceding text has been consumed. Admission rejections can
return without an event; fatal stream failure closes the scope and resolves any
pending prompt. A full queue that prevents terminal delivery fails both the
prompt and reader; no successful completion is reported without its event.
Transport failures produce port errors rather than fabricated domain completion
events. Stream exhaustion alone does not establish success.

Stop closes admission and pending permissions, sends ACP cancellation, and waits
within the configured grace period. It then closes stdin, allows harness teardown,
sends termination to the owned group if necessary, escalates to a forced kill,
reaps the direct child, and verifies group disappearance. Stderr is drained
without retaining provider text. Protocol writes also have a one-second bound;
permission cancellation is bounded by the grace period. Startup failures,
provider/transport failures, deadlines, dropped consumers, and shutdown share
this cleanup path.

Stop always closes this binding; open a new binding for later execution. Repeated
Stop calls return the same cleanup result. `Cancelled` is withheld until cleanup
succeeds. A known completion can win the race with Stop without being rewritten.
If cleanup cannot be established, the port reports `CleanupUncertain` and remains
closed. A provider error or idle protocol failure is distinct from successful
resource cleanup; the event reader and pending prompt preserve that failure.

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
Other smoke modes are `stop` (after first text), `deny-write`, `stop-write`
(while permission is pending), and `verify-write`. The last mode allows only one
Write to `nessa-binding-smoke.txt` in the selected workspace with exact content
`Nessa binding smoke test\n`; every other request is denied. Use a scratch workspace
for these file tests. Authentication/provider failures are surfaced as typed
errors, without provider error text that could contain credentials.

## Verification

On **2026-09-12, macOS**, the real Rust adapter with the pinned harness and exact
`claude-haiku-4-5-20251001` model completed these checks using existing local auth:

| Check | Observed result |
| --- | --- |
| Text prompt | Expected text; `Completed`; cleanup without force |
| One exact Write permission | One approval; exact file content verified; `Completed` |
| Denied Write | Denial observed; requested file absent |
| Stop after first streamed text | `Cancelled` after cleanup without force |
| Stop with pending Write permission | `Cancelled`; requested file absent |

Automated validation passes **57 SDK tests**, Clippy, formatting, and the declared
Rust 1.89 build. The domain coverage gate remains **364/364 lines, 60/60 functions,
and 445/445 regions**. The shared local-storage/auth/server test and Clippy checks
also pass; this change adds no domain exclusions.

The deterministic Python peer tests protocol faults, startup deadlines, permission
correlation, sparse patches, queue overflow, repeated Stop, completion races,
session isolation, dropped handles, and a TERM-resistant parent with a reaped
child. They are adapter/process tests, not provider compatibility evidence.
Application tests substitute independent session implementations without ACP.
The existing CI job runs SDK tests and Clippy on macOS/Linux/Windows; native
process tests run on Unix and need `/usr/bin/python3`. Live authenticated checks
are manual. Linux live-provider behavior, Windows process ownership, unrestricted
detached execution, crash recovery, and durable Conversation semantics are not
claimed by these macOS results.

Sources for the pinned behavior: [Claude ACP source](https://github.com/agentclientprotocol/claude-agent-acp/tree/c2e4815),
[custom model options](https://code.claude.com/docs/en/model-config#add-a-custom-model-option),
and [ACP prompt cancellation](https://agentclientprotocol.com/protocol/prompt-turn#cancellation).
