# Hook capability survey

Source survey for [#134](https://github.com/nessalabs/nessa-agent/issues/134).
First recorded 2026-09-22 against Nessa
`1b244f9be4cdc64c1298633422596a1a1e00fc6c`. Refreshed 2026-10-11 against Nessa
`8c1161697d9bf8c21b3bdb8bd9848a116de1f352` so the OpenCode binding matches
[#162](https://github.com/nessalabs/nessa-agent/pull/162) and the Codex binding
matches [#571](https://github.com/nessalabs/nessa-agent/pull/571) and
[#602](https://github.com/nessalabs/nessa-agent/pull/602). The harness pins
below did not move. The
[proposed decision](../../adr/todo/0014-nessa-owned-policy-hooks.md) owns
enforcement and degradation policy. This report inventories evidence; it is not
an implementation or live-provider certification.

## Versions and evidence

| Harness | Pinned input | Evidence and limits |
| --- | --- | --- |
| Claude | ACP 0.76.0; agent SDK 0.3.257 | Repository [lock](../../../crates/nessa-sdk/harnesses/claude-acp/package-lock.json), [ACP commit](https://github.com/agentclientprotocol/claude-agent-acp/tree/c2e4815029ef3962787ecaefe208b0b6f8b81302) and [SDK artifact](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.3.257.tgz). Native declarations do not establish ACP access. |
| Codex | ACP 1.12.0; Codex 0.154.0 | Repository [lock](../../../crates/nessa-sdk/harnesses/codex-acp/package-lock.json), [ACP commit](https://github.com/agentclientprotocol/codex-acp/tree/a7afd2ae077d625710194d9701b83595494449de) and Codex commit `6b9826e3aa83b1a5947db50f4332cb9c65f1b340`. |
| Opencode | 1.18.31 | Repository [release pins](../../../crates/nessa-server/data/agent-releases.json), upstream commit `014614d35b397775e5d397a490fc72368c894ec2`. Local Homebrew 1.18.20 was not used as evidence of pinned runtime behavior. |
| Kiro | No repository pin or adapter | Local signed CLI 2.22.0 installation/help/static strings; current official documentation is supplementary and not pinned behavior. No Nessa support claim. |

No authenticated model calls or arbitrary native hooks ran in this survey.
Source-supported, declaration-only, and runtime-verified are different evidence
classes. No entry below upgrades native support into a Nessa capability merely
because the event name resembles one Nessa observes.

## Claude target and native equivalents

The exact 33-event vocabulary comes from the pinned
[SDK declaration](https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.257/sdk.d.ts)
(`HOOK_EVENTS`, line 854). Every Claude entry is **declared**, not individually
exercised. Codex's [twelve native events](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/hooks/src/engine/mod.rs#L167-L181) are source-supported; a dash means absent
from that native event enum. Opencode entries are partial semantic equivalents:
`event` is fire-and-forget observation, not a veto. Kiro entries refer only to its
evidenced legacy native vocabulary; every Kiro ACP path remains unknown.

For Nessa ownership, **own** means it can define a local boundary from its own
commands/evidence (implementation still required); **report** needs provider
facts; **define** needs a product meaning before it can promise parity. A local
boundary is never silently presented as the provider's identically named event.

| Claude event | Codex native | Opencode native equivalent | Kiro legacy equivalent | Nessa ownership/degradation |
| --- | --- | --- | --- | --- |
| PreToolUse | Same | `tool.execute.before` | `preToolUse` | Own only at a proven execution gate; refuse blanket coverage. |
| PostToolUse | Same | `tool.execute.after` | `postToolUse` | Report completion; observation cannot rewrite completed effects. |
| PostToolUseFailure | — | Tool/error event | — | Report typed failure where available; do not infer from prose. |
| PostToolBatch | — | — | — | Define batch identity/ending; otherwise unsupported. |
| Notification | — | `event` | — | Own Nessa notifications separately; provider notification requires reporting. |
| UserPromptSubmit | Same | `chat.message` | `userPromptSubmit` | Own submitted-input boundary, with delivery mode explicit. |
| UserPromptExpansion | — | — | — | Report provider expansion; unsupported without it. |
| SessionStart | Same | Session event | `agentSpawn` (partial) | Own local open; provider startup is a separate fact. |
| SessionEnd | Same | Session event | — | Own local closure; do not assert external release without evidence. |
| Stop | Same | Session/step event | `stop` | Own settlement; local stop and provider completion remain distinct. |
| StopFailure | — | Error event | — | Own local failure evidence; do not fabricate provider failure. |
| SubagentStart | Same | Agent/tool event (partial) | — | Report stable subagent identity/lifecycle; otherwise unsupported. |
| SubagentStop | Same | Agent/tool event (partial) | — | Report correlated completion; tool title is insufficient. |
| PreCompact | Same | `experimental.session.compacting` | — | Report before-compaction boundary; otherwise unsupported. |
| PostCompact | Same | Compaction event/autocontinue | — | Report completed compaction; otherwise unsupported. |
| PreModelSwitch | — | — | — | Report before-switch boundary; config observations are too late. |
| PostModelSwitch | — | Model/config event (partial) | — | Report validated model change; no implied veto. |
| PermissionRequest | Same | Event; `permission.ask` is inactive | — | Own reviews actually requested; never all tools by assumption. |
| PermissionDenied | — | Permission event (partial) | — | Own Nessa decision, separately from provider delivery/effect. |
| Setup | — | — | — | Define; native setup is not Nessa gateway startup. |
| TeammateIdle | — | — | — | Define team lifecycle; otherwise unsupported. |
| TaskCreated | — | — | — | Define task identity; otherwise unsupported. |
| TaskCompleted | — | — | — | Define task result; otherwise unsupported. |
| Elicitation | — | Unverified | Unverified | Report actual forwarded request; capability remains unknown until proven. |
| ElicitationResult | — | Unverified | Unverified | Report correlated result; declaration alone insufficient. |
| ConfigChange | — | `config` / event (partial) | — | Own Nessa policy revisions; provider config needs reporting. |
| WorktreeCreate | — | Event (partial) | — | Define ownership; no fabricated native worktree hook. |
| WorktreeRemove | — | Event (partial) | — | Define removal/cleanup meaning; otherwise unsupported. |
| InstructionsLoaded | — | Input transforms (not same event) | — | Report exact load; a transformed prompt does not prove it. |
| CwdChanged | — | — | — | Define/provider report; otherwise unsupported. |
| FileChanged | — | File event (partial) | — | Define observation scope; no general filesystem guarantee. |
| DirectoryAdded | — | — | — | Define/provider report; otherwise unsupported. |
| MessageDisplay | — | — | — | Define surface acknowledgement; generation is not display. |

Codex additionally declares `Interrupt`; it is not one of Claude's 33 names.
Opencode additionally exposes model parameter/header/system-message transforms,
tool definitions, shell environment, custom tool/auth/provider registration and
plugin disposal. These are not automatically importable Nessa policy powers.

## What the bindings actually expose

### Claude

The pinned SDK declares `allow | deny | ask | defer`, common `continue`,
`stopReason`, `decision`, `systemMessage`, and event-specific input/context
outputs. Matchers and timeouts are declared. Base input includes session,
transcript path and working directory, with optional prompt, permission mode and
agent identity/type. A transcript path is not a bounded, authorized context view.

Claude ACP [uses `canUseTool`](https://github.com/agentclientprotocol/claude-agent-acp/blob/c2e4815029ef3962787ecaefe208b0b6f8b81302/src/acp-agent.ts#L6903-L6940) to request ACP permission and
[installs its own internal hooks](https://github.com/agentclientprotocol/claude-agent-acp/blob/c2e4815029ef3962787ecaefe208b0b6f8b81302/src/acp-agent.ts#L7901-L8158).
JSON metadata cannot carry arbitrary JavaScript callbacks. The adapter does not
request SDK hook-event reporting. Nessa's [profile](../../../crates/nessa-sdk/src/infrastructure/claude_acp/sessions/profile.rs)
requests `disableAllHooks: true` and empty `settingSources` for native configured
hooks. The negotiated fact stays `Unknown`: this profile has no checked answer
for the effective setting, and a requested override is not reported as supported.
`claude_startup_and_restoration_continue_when_native_hook_suppression_is_unknown`
reads that request from the fixture and runs a prompt, a restore, and a second
prompt while the fact stays unknown. Neither the setting nor the SDK event enum
exposes 33 enforceable Nessa hooks through ACP. Reviewed tools can receive a
selected rejection; tools not routed through that gate do not gain a veto
through a later tool notification. Which of the 33 events fire on a turn, which
output combinations the CLI accepts, and whether a deny stops a tool were not
run: they need the pinned Claude CLI and a live model. The fixture does not
emit those events, and this survey does not treat that silence as the CLI.

The packaged native implementation contains conditional park/resume handling for
`defer`, so the earlier hypothesis in #134 that it simply means no opinion is not
established. That native path is not an ACP permission-deferral contract. No
reviewed ACP outcome carries a later answer to a resolved request. Its exact
native per-event behavior remains untested here; import refuses it as a Nessa
permission-deferral output.

Native failure behavior also depends on the execution path. SDK pre-tool callback
failure/timeout has blocking handling in packaged source, whereas executable
command hooks use event/exit-status rules. A single provider-wide fail-open or
fail-closed flag would erase this distinction; Nessa must enforce its own default.

### Codex

Native hooks use Claude-shaped configuration and
[output validation](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/hooks/src/engine/output_parser.rs#L369-L510) but reject unsupported
combinations. Pre-tool output supports denial and an allowance with rewritten
input; `ask` and legacy `approve` are not accepted there. Permission hooks accept
allow/deny; no decision falls through to ordinary approval. Native hook failure,
timeout and malformed JSON generally record failure and continue; explicit
supported blocking output is different. That default cannot implement Nessa's
fail-closed enforcement policy unchanged.

The ACP adapter [discards `hook/started` and `hook/completed`](https://github.com/agentclientprotocol/codex-acp/blob/a7afd2ae077d625710194d9701b83595494449de/src/CodexEventHandler.ts#L604-L605)
and [drops hook history entries](https://github.com/agentclientprotocol/codex-acp/blob/a7afd2ae077d625710194d9701b83595494449de/src/ResponseItemHistoryFallback.ts#L182).
Native hooks are therefore not an observable Nessa hook stream.
The adapter separately handles `thread/compacted` and `model/rerouted` in the
[same event handler](https://github.com/agentclientprotocol/codex-acp/blob/a7afd2ae077d625710194d9701b83595494449de/src/CodexEventHandler.ts#L572-L583);
these observations do not expose pre-compaction or general model-switch vetoes.

#### Native suppression request and verification gap

Current main asks for suppression on every launch and does not treat the ask as
proof. The [binding](../../../crates/nessa-sdk/src/infrastructure/codex_acp/sessions/binding.rs)
puts `features.hooks: false` and `notify: []` in `CODEX_CONFIG` beside the model
and, when the binding has them, the instructions. The pinned adapter,
`@agentclientprotocol/codex-acp` 1.12.0, forwards that JSON as the `thread/start`
config override. A negotiated session reports native hook suppression as
`Unsupported`. Startup and restoration still complete, and a prompt runs both
times.
`codex_startup_and_restoration_continue_when_native_hook_suppression_is_unsupported`
reads the request the fixture recorded and is that contract. It does not close
[#147](https://github.com/nessalabs/nessa-agent/issues/147).

The upstream pin is still Codex 0.154.0 at
`6b9826e3aa83b1a5947db50f4332cb9c65f1b340`. Rechecked there, the request and the
effective setting stay different:

- Codex's [engine](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/hooks/src/engine/mod.rs#L228-L267)
  removes ordinary configured handlers when the feature is false, retaining
  provider builtin cleanup. Its [regression test](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/hooks/src/engine/mod_tests.rs#L1967-L2000)
  distinguishes builtin cleanup from ordinary trusted plugin commands. This is
  upstream source/test evidence; the test was not run here. Disabling all plugins
  to remove cleanup would change broader capabilities and is not this contract.
- Legacy [`notify` dispatch](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/hooks/src/registry.rs#L118-L180)
  is constructed independently of the hook feature. Turning off only the feature
  leaves this command path available.

The [configuration loader](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/config/src/loader/mod.rs#L440-L497)
applies session overrides after ordinary user/project layers, then applies legacy
managed file and MDM layers above those overrides. Those later layers can restore
hooks or notification commands. By contrast, conflicting modern feature
requirements [reject configuration](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/core/src/config/managed_features.rs#L280-L319).
Modern rejection does not prove safety against the different legacy path.

ACP [merges the environment configuration into session options](https://github.com/agentclientprotocol/codex-acp/blob/a7afd2ae077d625710194d9701b83595494449de/src/CodexAcpClient.ts#L721-L769)
for new and restored sessions. Codex's [resume path](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/app-server/src/request_processors/thread_processor.rs#L3652-L3689)
returns an already-running thread before applying new configuration. Nessa
opens a fresh process per binding, so its normal restoration is cold;
future process reuse must not assume configuration is reapplied.

ACP exposes no effective hooks/notify attestation, and its discarded hook events
cannot prove their absence. Nessa's profile verification checks model and mode,
not suppression. An isolated `CODEX_HOME` would relocate authentication and
persisted sessions without removing system/MDM configuration precedence; it is
not a demonstrated fix. On 2026-09-22 the proposed decision required suppression
to be established before startup, and this survey said startup should be refused
until that proof existed. That refusal is not the current decision.
[ADR 0014](../../adr/todo/0014-nessa-owned-policy-hooks.md) reports unproven
suppression and continues startup and restoration, including with zero Nessa
hooks. The current profile reports `Unsupported` for that gap. It does not
report `Unknown`, and the request does not make the fact `Supported`. It does
not close [#147](https://github.com/nessalabs/nessa-agent/issues/147) by
claiming proof.
No attestation mechanism is claimed implemented in this survey. The loader
order above is why the negotiated fact stays unsupported after the request was
added. Which of the twelve native events fire on a turn, and whether a parsed
output actually stops a tool or reaches the model, were not run: the fixture is
not the Codex process, and those rows need a live provider. The pinned parser's
own unit tests were not executed in this repository.

### Opencode

The [plugin interface](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/plugin/src/index.ts#L222-L334)
at OpenCode 1.18.31 uses mutable output objects. The [dispatcher](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/opencode/src/plugin/index.ts#L170-L297)
awaits transforms sequentially and propagates rejection, but ignores/logs
initialization, config, and disposal failures and does not await generic `event`
hooks. No equivalent built-in matcher, timeout, or Claude decision result exists.
`permission.ask` is declared on that interface and has no `trigger` call site in
this pin; the only matches are the type and the separate `permission.asked`
event. It must not be credited with allow/deny power.

Call sites that do exist, rechecked at the same commit:

| Hook | Where the return goes |
| --- | --- |
| `tool.execute.before` | [tools.ts](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/opencode/src/session/tools.ts#L106-L111), the task path, and code-mode discard the returned output and then execute the original `args`. An in-place edit of that object is visible because the output holds the same reference. Replacing `output.args` is not read back. A rejection fails the effect before execute. |
| `tool.execute.after` | The same object is what the tool path returns afterwards, so in-place edits of title, output, and metadata stick. The returned wrapper is not re-read. |
| `chat.message`, `command.execute.before`, `experimental.chat.messages.transform`, `experimental.chat.system.transform` | The caller keeps using the objects it passed in. In-place edits stick. Replacing `output.message`, `output.parts`, or `output.system` is not read back. |
| `chat.params`, `chat.headers`, `shell.env`, `experimental.session.compacting`, `experimental.compaction.autocontinue`, `experimental.text.complete`, `experimental.provider.small_model` | The caller uses the trigger's return value. |
| `tool.definition` | The caller reads the same output object after the trigger, so a replaced description or schema is what gets registered. |
| `event` | Fired without awaiting. It cannot veto. |
| `config`, plugin init, `dispose` | Failures are logged and ignored. |

`permission.ask` is declared on the interface and has no `trigger` call site in
this pin. `auth` is collected from the plugin list by the provider auth loader;
that path is not `Plugin.trigger`.

None of those call sites were executed here. Which of them fire on a turn, and
whether an in-place edit is what the model sees, need the pinned binary and a
live model. The ACP fixture does not emit plugin callbacks.

Nessa's [binding](../../../crates/nessa-sdk/src/infrastructure/opencode_acp/sessions/binding.rs),
after [#162](https://github.com/nessalabs/nessa-agent/pull/162), gives the
process a private `HOME` and XDG roots, removes `OPENCODE_CONFIG`,
`OPENCODE_CONFIG_CONTENT`, and `OPENCODE_CONFIG_DIR`, and sets
`OPENCODE_DISABLE_PROJECT_CONFIG=1` and `OPENCODE_PURE=1`. `PURE` empties the
external plugin list. It does not disable the default plugins compiled into
the binary. The negotiated fact stays `Unknown`: the profile has no checked
answer, and this request is not reported as supported.
`opencode_startup_and_restoration_continue_when_native_hook_suppression_is_unknown`
reads the request from the fixture and runs a prompt, a restore, and a second
prompt while the fact stays unknown. The ignored test
`pinned_binary_cannot_load_caller_config_tools_mcp_or_native_hooks` is the
check against the real 1.18.31 binary, and it runs only when
`NESSA_PINNED_OPENCODE_BINARY` is set. It was not run for this refresh.

ACP permissions apply only when Opencode's policy first chooses `ask`. Its
[permission bridge](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/opencode/src/acp/permission.ts#L51-L96)
rejects a failed client call and any reply other than `once` or `always`.
Current Nessa configuration is deny-first with selected tools allowed, not an
ask gate for every tool. Native argument and result transforms are unavailable
over that ACP boundary.

[#147](https://github.com/nessalabs/nessa-agent/issues/147) no longer tracks
Opencode; that part was [#162](https://github.com/nessalabs/nessa-agent/pull/162).
No claim of complete tool interception, default-plugin silence, filesystem
secrecy, or startup containment follows from `PURE` and the private roots
alone. The limits that remain are on the binding's `SESSION_POLICY` constant.

### Kiro

Kiro is installed here but not pinned or integrated by Nessa. Local CLI 2.22.0
help offers ACP; static strings include legacy `agentSpawn`, `userPromptSubmit`,
`preToolUse`, `postToolUse`, and `stop`. The installed chat binary SHA-256 is
`0dfe351ac7afd99a16210b66429e255d3f13fa0701bce7a9225ee88e71f1b58f`.
No handshake or authenticated call ran.

Current [configuration docs](https://kiro.dev/docs/custom-agents/configuration-reference/)
describe a CLI 3.0 migration, while [action docs](https://kiro.dev/docs/hooks/actions/)
describe command exit behavior and newer formats. They cannot establish the
installed 2.22 runtime's timeout, rewrite or ACP forwarding contract. Native
pre-tool/prompt blocking and context injection are documented; Nessa reachability
remains unknown. Compiled `elicitation/create` symbols are not forwarding proof.

## Context and lifecycle evidence at the requested points

The following tables describe native inputs, not a Nessa hook payload or proof
of ACP delivery. Claude fields come from the pinned
[SDK types](https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.257/sdk.d.ts);
Codex fields come from its pinned
[generated command-input schemas](https://github.com/openai/codex/tree/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/hooks/schema/generated);
Opencode signatures come from its pinned
[plugin interface](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/plugin/src/index.ts#L222-L334).
The links contain the full shapes; the table highlights the context needed to
compare these boundaries. None of these tables reports an exercised model or tool.

Claude's common input requires session ID, transcript path and working directory.
Prompt correlation, permission mode and agent identity/type are optional; effort
is conditional on a supported model and tool-use context, not session lifecycle.
A path is not inline transcript content. Codex schemas require a transcript-path
field whose value may be null. Its tool/prompt/stop inputs carry a turn ID; its
session-start/end inputs do not. Optional subagent fields on tool/prompt inputs do
not establish a subagent identity on every event.

| Requested point | Claude native context | Codex native context | Opencode native context |
| --- | --- | --- | --- |
| Before tool | Common fields plus tool name, input and tool-use ID. | Session/turn, cwd, model/mode, transcript path, tool name/input/use ID; optional agent identity/type. | Tool/session/call IDs; mutable arguments are the separate output object. |
| After tool | Before-tool fields plus response and optional execution duration. | Before-tool fields plus tool response. | Tool/session/call IDs and arguments; mutable title, output and metadata. |
| Prompt submitted | Common fields plus prompt; optional source and session title. | Session/turn, cwd, model/mode, transcript path and prompt; optional agent identity/type. | `chat.message` receives session plus optional agent/model/message/variant; output carries message and parts. |
| Stop | Common fields plus stop-hook-active; optional last assistant message and background-work context. | Session/turn, cwd, model/mode, transcript path, stop-hook-active and nullable last assistant message. | No dedicated equivalent callback; generic session/step events are observations. |
| Session start/resume | Common fields plus source (`startup`, `resume`, `clear`, `compact`, `fork`); optional model/title and resume/cache estimates. | Session, cwd, transcript path, model/mode and source (`startup`, `resume`, `clear`, `compact`). | Generic session events; ACP restoration is a separate handler path, not a dedicated plugin resume callback. |
| Session end | Common fields plus exit reason. | Session, cwd, transcript path and reason; no turn/model/mode fields in the command schema. | Generic session events and plugin disposal have different owners; disposal is not a per-session end guarantee. |
| Notification/permission | Notification message/type and optional title; permission requests instead carry tool name/input and optional suggestions. | Permission request carries session/turn, cwd, model/mode, transcript path and tool name/input, with optional agent fields; no general Notification event. | Generic event payload; the declared permission input/output callback is inactive at this pin. |

Kiro has no repository pin or adapter, so context, resume, output and failure
behavior at every requested point remain unknown. Its local strings/help are not
an equivalent input schema or a runtime test.

`UserPromptSubmit` is an input boundary, not proof that a model turn began; `Stop`
is a native lifecycle hook, not automatically the same fact as Nessa settlement.
Likewise Opencode's `chat.message` and ACP idle status must not be relabelled as
a universal turn-start/end hook. Nessa maps its own submission and settlement
boundaries explicitly, retaining delivery mode and provider evidence separately.

### Restoration is not one native event

Claude declares a `SessionStart` resume source and optional elapsed-time,
context-token and cache-estimate fields. Their presence in the SDK type does not
prove delivery through Nessa's ACP session restoration.

Codex's [cold-resume initialization](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/core/src/session/session.rs#L1616-L1640)
queues `SessionStart(resume)`; [pending startup hooks](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/core/src/hook_runtime.rs#L123-L151)
run at the next turn. An [already-running thread](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/core/src/thread_manager.rs#L1937-L1962)
returns without that spawn path. A successful resume response is therefore not
proof that the native startup hook has run, nor that both resume modes emit it.

Opencode's [load and resume handlers](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/opencode/src/acp/service.ts#L211-L337)
both restore session state and register supplied MCP servers. Load fetches and
replays messages; resume fetches up to 20 messages to restore state and does not
replay them. Neither handler exposes a dedicated plugin resume callback. These
are pinned source observations, not exercised restoration tests.

### Controlled Claude SDK callback bridge probe

On 2026-09-22, an isolated fixture exercised the actual SDK `0.3.257` JavaScript
bridge with an owned fake CLI selected explicitly by
`pathToClaudeCodeExecutable`. The registry tarball SHA-256 was
`ccc63d1abbf816d30a242f8c76e006b082180910c2220916c47d44e53c8426c0`.
The runner cleared the inherited environment, supplied fresh scratch home/config
and temporary directories, disabled setting sources and session persistence, and
made no model or authenticated provider call. The coordinating reviewer inspected
the fixture and independently reran its assertions successfully. This is a
record of a local controlled probe, not a repository regression suite.

| Controlled callback | Exercised bridge result |
| --- | --- |
| PreToolUse | Input and separate tool-use ID reached the callback; returned deny, rewrite/context fields and generic message fields were serialized unchanged with the original request ID. |
| SessionStart | Supplied resume source and timing/token/cache fields reached the callback; returned context/title fields retained request correlation. |
| Stop | Returned `continue: false`, stop reason and system message were serialized unchanged. |
| PostToolUse callback throws | SDK serialized a correlated error containing the controlled failure reason. |

The fake peer used callback IDs registered by the SDK during initialization and
supplied the event frames itself. Transporting output fields together does not
prove the native runtime accepts that combination. This probe establishes neither
actual tool veto/rewrite nor model-visible context, native event timing, matcher
or timeout behavior, command-hook behavior, ACP forwarding or real session-resume
lifecycle delivery. Those consuming boundaries are the live-provider rows
below. The probe was not turned into a regression test: a fake CLI that supplies
the event frames would be a claim about the CLI.

### What the contract tests pin, and what needs a live provider

The tests use the existing ACP fixtures. Each fixture accepts the launch only
after it has seen that binding's suppression request, and writes `hook-request`
for the test to read. A prompt completes, the session is restored, and a prompt
completes again. The negotiated fact is the profile's checked answer.

| Harness | Events that fire | Output the harness accepts | Native-hook suppression |
| --- | --- | --- | --- |
| Claude ACP 0.76.0, SDK 0.3.257 | Not run. The adapter does not request SDK hook-event reporting, and the fixture does not emit hook events. Which of the 33 declared events fire on a turn needs the pinned CLI and a live model. | Not run. SDK 0.3.257 declares `allow \| deny \| ask \| defer`, `continue`, `stopReason`, `decision`, `systemMessage`, and `hookSpecificOutput`. The CLI's acceptance of a combination needs that live process. The four-callback probe above is bridge serialization, not CLI acceptance. | `claude_startup_and_restoration_continue_when_native_hook_suppression_is_unknown`. The fixture records `disableAllHooks: true` and empty `settingSources`. The negotiated fact stays `Unknown`. The open, the prompt, the restore, and the second prompt complete. |
| Codex ACP 1.12.0, Codex 0.154.0 | Not run. The adapter discards `hook/started` and `hook/completed`. Which of the twelve native events fire on a turn needs a live provider. | Not run as behavior. The pinned parser rejects `permissionDecision:ask`, legacy `decision:approve`, and `continue: false` on pre-tool, and rejects a deny without a reason. Those upstream unit tests were not executed here, and parser acceptance is not a tool that stayed unrun. | `codex_startup_and_restoration_continue_when_native_hook_suppression_is_unsupported`. The fixture records `features.hooks: false` and `notify: []`. The negotiated fact stays `Unsupported`. The open, the prompt, the restore, and the second prompt complete. |
| OpenCode 1.18.31 | Not run. The call-site table above is source. `permission.ask` has no trigger. Which of the triggered hooks fire on a turn needs the pinned binary and a live model. | Not run. The same table says which call sites use the trigger's return and which keep the original object. An in-place edit reaching the model was not observed. | `opencode_startup_and_restoration_continue_when_native_hook_suppression_is_unknown`. The fixture records `OPENCODE_PURE=1`, project config disabled, the three alternate config inputs absent, and a home other than the caller's. The negotiated fact stays `Unknown`. The open, the prompt, the restore, and the second prompt complete. Default plugins inside the binary are outside that request. `pinned_binary_cannot_load_caller_config_tools_mcp_or_native_hooks` checks the real binary and is ignored unless `NESSA_PINNED_OPENCODE_BINARY` is set. |

Kiro has no repository pin and no adapter. Pinning it is not part of this
refresh. Its local strings and help remain observations, not a behavior test.

A callback result or parser acceptance alone cannot prove that a tool did not run
or that the model received injected text. Those claims stay unmade until the
consuming execution boundary is checked against a live provider.

## Output support and explicit degradation

| Requested power | Native evidence | Nessa contract/degradation |
| --- | --- | --- |
| Allow/deny/ask | Claude declares all; Codex accepts event-specific subset; Opencode declared permission callback inactive | Select only an actual offered option at a proven gate. Ask means a pending Nessa review; never bypass enforcing policy. |
| Defer | Claude declares conditional native handling; ACP has no later-answer outcome | Unsupported permission deferral; do not emulate with steering. |
| Approve/block | Claude declares; Codex validates by event and rejects some combinations | Import only with exact event-specific mapping; otherwise unsupported. |
| Updated input | Claude/Codex pre-tool parsers accept a rewrite with an allow. Opencode's `tool.execute.before` call sites discard the returned output and execute the original `args`; only an in-place edit of that object is visible in source, and it was not executed | Unsupported on a binding lacking a proven rewrite-before-execution path. |
| Additional context/system message | Native event-specific Claude/Codex outputs and Opencode transforms | Refuse required model delivery unless that exact boundary delivers it; UI text is a separate lesser behavior. |
| Stop/end turn/close session | Native output and local lifecycle are different authorities | Use attributed local stop; require proven target fencing, cleanup and reuse behavior. No synthetic provider terminal result. |
| Matcher | Claude/Codex native matching; no Opencode dispatcher matcher | Nessa-owned bounded matcher over validated tool identity; provider aliases need explicit translation evidence. |
| Timeout/on-error | Different native defaults; Kiro installed timeout unverified | Nessa owns bounded execution and fail-closed enforcement; import reports semantic differences. |
| Thinking/transcript | Native inputs can name/read wider transcripts | Default exclude thinking; explicit disclosure opt-in; bounded correlated invocation view only. |

## First capability declarations and evidence still required

Extend `OperationCapabilities` through the existing gateway/client projection.
Every feature needs its own tri-state value, scope and explanation; the existing
global `negotiated` bit cannot turn a native API declaration into support.

### Question evidence belongs to different revisions

| Evidence owner | Exact scope | What it establishes |
| --- | --- | --- |
| Historical Nessa baseline | `1b244f9be4cdc64c1298633422596a1a1e00fc6c`, SDK source | Had no question/elicitation advertisement or incoming question implementation. |
| Merged question path | [PR #215](https://github.com/nessalabs/nessa-agent/pull/215), ACP subprocess and gateway/panel tests | With tools enabled, the pinned Claude ACP profile advertises `elicitation.form: {}`. Its agent question reaches the conversation and a correlated answer returns. Codex and Opencode do not advertise questions. |
| Historical WS7 isolated probes | Claude ACP 0.76.0 / Codex ACP 1.12.0 handlers and installed ACP 1.4.0 schema, controlled connection | Tests malformed boolean and valid object inputs, field forwarding and correlated handler return only. No model/MCP/app-server/panel end-to-end proof. |

The probe report says both pinned schema readers discard boolean `form: true`;
`form: {}` is the valid advertisement. Claude forwards form fields and accepts
correlated content in its direct handler probe; Codex forwards and returns
accepted content with the object capability. This is a historical probe report,
not a test of the historical baseline. Opencode's pinned
[MCP initialization](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/opencode/src/mcp/index.ts#L38-L82)
does not enable elicitation. The merged question path establishes incoming agent
questions for Claude, not MCP server elicitation forwarding. The latter remains
unknown until a configured server's request completes the correlated path.
The outbound
MCP ask proposal [#146](https://github.com/nessalabs/nessa-agent/issues/146) is
closed as not planned and deferred; it is not part of this implementation plan.

| Capability | Baseline answer | Activation requirement |
| --- | --- | --- |
| Native executable-hook suppression | Claude sends `disableAllHooks: true` and reports `Unknown`. Codex sends `features.hooks: false` and `notify: []` and reports `Unsupported`, because legacy managed configuration and MDM still apply after the session override. OpenCode sends `OPENCODE_PURE=1` with private roots and project config disabled, and reports `Unknown`; default plugins stay loaded | Report unproven suppression and continue startup and restoration, including with zero Nessa hooks. Do not report supported without effective proof. Preserve provider builtin cleanup. |
| Pre-tool denial | Conditional ACP review path, not universal coverage | Prove the requested tool set cannot execute before Nessa's decision. |
| Policy end-turn | Not implemented | Same target across direct/queued/steered work; late provider result and reusable session verified. |
| Policy session close | Existing attributed close primitive, configured policy integration absent | Rule cause retained through all cleanup/audit paths. |
| Compaction reporting | Native source evidence; Nessa mapping unverified | Actual correlated event from the selected binding; otherwise unsupported. |
| Model-switch reporting | Config facts are not necessarily switch lifecycle | Validate actual event/phase and current model together. |
| MCP elicitation forwarding | Incoming agent questions are implemented for Claude; MCP server forwarding remains unverified | A configured server's request must reach the question path and return correlated accept/decline/cancel; declarations alone insufficient. No Nessa-originated ask tool is implied. |
| Permission deferral | Unsupported on these third-party bindings | No steering workaround; future own-loop harness outside scope. |

Test obligations for implementation include unsupported/unknown rejection before
effects; renegotiation/restoration; causal-context lag and truncation; mixed hook
verdict precedence; native suppression with zero configured hooks; once-only
remembered decisions with revocation; late provider outcomes; caller loss; and
audit-sink failure while cleanup still runs. These are requirements for the
proposed policy-hook integration, not tests claimed to have run. The deferred
outbound proposal supplies no implementation or support claim.
