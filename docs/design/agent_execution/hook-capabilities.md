# Hook capability survey

Source survey for [#134](https://github.com/nessalabs/nessa-agent/issues/134),
recorded 2026-09-22 against Nessa `1b244f9be4cdc64c1298633422596a1a1e00fc6c`.
The [proposed decision](../../adr/todo/0014-nessa-owned-policy-hooks.md) owns
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
requests `disableAllHooks: true` for native configured hooks. Neither this setting
nor the SDK event enum exposes 33 enforceable Nessa hooks through ACP. Reviewed
tools can receive a selected rejection; tools not routed through that gate do not
gain a veto through a later tool notification. Whole-surface native suppression
and all output combinations require version-specific behavior tests.

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

The reviewed Nessa baseline's
[binding](../../../crates/nessa-sdk/src/infrastructure/codex_acp/sessions/binding.rs)
sets model and optional instructions in `CODEX_CONFIG`; it does not request hook
suppression. At the pinned upstream versions, adding `features.hooks: false` and
`notify: []` is a source-supported suppression request, not an implemented Nessa
feature or proof of effective suppression. These are separate settings:

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
returns an already-running thread before applying new configuration. Nessa's
baseline opens a fresh process per binding, so its normal restoration is cold;
future process reuse must not assume configuration is reapplied.

ACP exposes no effective hooks/notify attestation, and its discarded hook events
cannot prove their absence. Nessa's profile verification checks model and mode,
not suppression. An isolated `CODEX_HOME` would relocate authentication and
persisted sessions without removing system/MDM configuration precedence; it is
not a demonstrated fix. Under ADR 0014, the minimal enforceable contract requires
suppression established before startup/restoration effects, including with zero
Nessa hooks. The current binding cannot satisfy that contract in the legacy
managed-policy scenario. Its suppression capability remains unknown, and required
startup must be refused until effective suppression can be established; adding
the request alone cannot close [#147](https://github.com/nessalabs/nessa-agent/issues/147).
No startup refusal or attestation mechanism is claimed implemented here.

### Opencode

The [plugin interface](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/plugin/src/index.ts#L222-L334)
uses mutable output objects. The [dispatcher](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/opencode/src/plugin/index.ts#L170-L297)
awaits transforms sequentially and propagates rejection, but ignores/logs
initialization/config/disposal failures and does not await generic event hooks.
No equivalent built-in matcher/timeout/Claude decision result exists.
The [tool boundary](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/opencode/src/session/tools.ts#L96-L130)
can mutate arguments before execution and result text/metadata afterwards.
`permission.ask` is declared but has no runtime trigger call site in this pin;
it must not be credited with allow/deny power.

Nessa's [binding](../../../crates/nessa-sdk/src/infrastructure/opencode_acp/sessions/binding.rs)
sets `OPENCODE_PURE=1` to disable external plugins. ACP permissions apply only
when Opencode's policy first chooses `ask`; its [permission bridge](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/opencode/src/acp/permission.ts#L35-L96)
rejects failed, cancelled or unusable client replies. Current Nessa configuration
is deny-first with selected tools allowed, not an ask gate for every tool.
Native argument/result transforms are unavailable over that ACP boundary.

Global configuration, custom tools that bypass permission evaluation, and MCP
startup are separate containment concerns documented in the binding. Work in
[#123](https://github.com/nessalabs/nessa-agent/issues/123) is not incorporated
into this baseline. [#147](https://github.com/nessalabs/nessa-agent/issues/147)
tracks the native-hook suppression gap; its Opencode work shares #123's owner.
No claim of complete tool interception, plugin/config
isolation, filesystem secrecy or startup containment follows from `PURE` alone.

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

## Output support and explicit degradation

| Requested power | Native evidence | Nessa contract/degradation |
| --- | --- | --- |
| Allow/deny/ask | Claude declares all; Codex accepts event-specific subset; Opencode declared permission callback inactive | Select only an actual offered option at a proven gate. Ask means a pending Nessa review; never bypass enforcing policy. |
| Defer | Claude declares conditional native handling; ACP has no later-answer outcome | Unsupported permission deferral; do not emulate with steering. |
| Approve/block | Claude declares; Codex validates by event and rejects some combinations | Import only with exact event-specific mapping; otherwise unsupported. |
| Updated input | Claude/Codex pre-tool and Opencode native mutation | Unsupported on a binding lacking a proven rewrite-before-execution path. |
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
| Reviewed Nessa baseline | `1b244f9be4cdc64c1298633422596a1a1e00fc6c`, SDK source | No question/elicitation advertisement or incoming question implementation. Provider handler capabilities do not make this baseline accept questions. |
| External unmerged `agent-questions` branch | [`73b826cf`, worker initialization](https://github.com/nessalabs/nessa-agent/blob/73b826cf0461d3aab8081869235b7f480fae52a1/crates/nessa-sdk/src/infrastructure/acp/executions/worker.rs#L679-L680) | Sends `elicitation.form: true`; this is external branch behavior, not baseline behavior or merged support. |
| Historical WS7 isolated probes | Claude ACP 0.76.0 / Codex ACP 1.12.0 handlers and installed ACP 1.4.0 schema, controlled connection | Tests malformed boolean and valid object inputs, field forwarding and correlated handler return only. No model/MCP/app-server/panel end-to-end proof. |

The probe report says both pinned schema readers discard boolean `form: true`;
`form: {}` is the valid advertisement. Claude forwards form fields and accepts
correlated content in its direct handler probe; Codex forwards and returns
accepted content with the object capability. This is a historical probe report,
not a test of the reviewed Nessa baseline or evidence that the external branch's
other defects are repaired. Opencode's pinned
[MCP initialization](https://github.com/anomalyco/opencode/blob/014614d35b397775e5d397a490fc72368c894ec2/packages/opencode/src/mcp/index.ts#L38-L82)
does not enable elicitation. No current whole-path support claim follows from
the isolated probes. Incoming schema/free-text acceptance and prerequisite repair
are tracked under [#151](https://github.com/nessalabs/nessa-agent/issues/151) within
#130. The external `agent-questions` owner supplies implementation evidence;
WS6 owns this matrix and defers question-dependent integration. The outbound
MCP ask proposal [#146](https://github.com/nessalabs/nessa-agent/issues/146) is
closed as not planned and deferred; it is not part of this implementation plan.

| Capability | Baseline answer | Activation requirement |
| --- | --- | --- |
| Native executable-hook suppression | Claude requests suppression; Codex baseline omits it and has an unresolved managed-policy precedence gap; Opencode uses PURE with separate containment gaps | Establish effective suppression before startup/restoration, even with zero Nessa hooks; preserve provider builtin cleanup. |
| Pre-tool denial | Conditional ACP review path, not universal coverage | Prove the requested tool set cannot execute before Nessa's decision. |
| Policy end-turn | Not implemented | Same target across direct/queued/steered work; late provider result and reusable session verified. |
| Policy session close | Existing attributed close primitive, configured policy integration absent | Rule cause retained through all cleanup/audit paths. |
| Compaction reporting | Native source evidence; Nessa mapping unverified | Actual correlated event from the selected binding; otherwise unsupported. |
| Model-switch reporting | Config facts are not necessarily switch lifecycle | Validate actual event/phase and current model together. |
| MCP elicitation forwarding | Nessa incoming path absent in baseline; full provider path unverified | A configured server's request must reach the question path and return correlated accept/decline/cancel; declarations alone insufficient. No Nessa-originated ask tool is implied. |
| Permission deferral | Unsupported on these third-party bindings | No steering workaround; future own-loop harness outside scope. |

Test obligations for implementation include unsupported/unknown rejection before
effects; renegotiation/restoration; causal-context lag and truncation; mixed hook
verdict precedence; native suppression with zero configured hooks; once-only
remembered decisions with revocation; late provider outcomes; caller loss; and
audit-sink failure while cleanup still runs. These are required tests, not tests
claimed to have run. #151's external owner supplies incoming-question evidence;
#141 approval work and question-dependent integration require a coordinated
handoff. The deferred outbound proposal supplies no implementation or support claim.
