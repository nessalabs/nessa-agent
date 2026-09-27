# 231. A conversation carries its own model and tool approval mode

## Purpose

People choose which model a conversation runs on, and how much its agent may do
without asking, from the composer. Today both are fixed for every conversation
of an agent. This record defines the intended wire contract, where each choice is owned,
how the three approval modes map onto each agent's own modes, and what happens
when the mode changes mid-conversation.

- **Date:** 2026-09-26
- **Status:** accepted
- **Issue:** [#231](https://github.com/nessalabs/nessa-agent/issues/231)

## Context

- **One model per agent.** `agents.runtimes.<agent>.model` builds one provider
  per agent (`composition/agent.rs`, `build::provider`), with the model baked into
  it. Sessions already run one process each (`acp/sessions/binding.rs`), so a
  model per session costs no new process model. It only has to reach the binding.
- **Creation is usually lazy.** The panel opens a tab locally and sends
  `conversation.create` on the first send (`gateway/effects.ts`). Staging an
  image creates the server conversation earlier because the upload needs an
  owner. The model becomes fixed at either creation point; choosing another
  model after staging opens a separate draft and keeps the staged draft intact.
- **Nessa bindings pin their configured mode and refuse a change.**
  Claude runs `default` with the reviewed-tools ask rule and deny list, Codex
  runs `read-only`, and OpenCode runs `plan` (`*_acp/sessions/profile.rs`). A
  `current_mode_update` to anything else is a protocol error.
- **The client knows no catalog.** `models.json` is read only by the server, and
  the panel knows only the agent picked at setup.

The binding constraint is [gate 13](../../../CODING_STANDARDS.md#gates). Which
models an agent may run, and which approval modes it can honour, are each owned
once, by the gateway and the SDK binding. The panel reads them; it never retypes
them.

## Initial ACP spike — 2026-09-26

A direct JSON-RPC-over-stdio spike on macOS exercised the installed Claude ACP
**0.76.0**, Codex ACP **1.12.0**, and OpenCode **1.18.31**. These were real
harnesses, bypassing Nessa's Rust bindings. Claude and Codex used existing local
authentication and environment. OpenCode used private HOME/XDG roots, disabled
project configuration and plugins, and a fixed deny-first launch permission
policy allowing reads. Prompts requested only a scratch file write; the probe
cancelled permission requests. No shell, network, MCP, or deny-list enforcement
claim follows from those file tests.

| Probe | Observed result |
| --- | --- |
| Claude mode selection | `default`, `acceptEdits`, and `bypassPermissions` accepted and reported back; return to `default` accepted. |
| Claude Write without an explicit ask rule | `default` requested approval; cancellation left the file absent. `acceptEdits` and `bypassPermissions` wrote without a permission request. |
| Claude Write with Nessa's `permissions.ask: ["*"]` | All three modes requested approval; cancellation left the files absent. Changing the mode alone does not remove the blanket ask rule. |
| Codex proposed mode IDs | `auto` and `full-access` rejected with JSON-RPC `-32602`. |
| Codex advertised mode IDs | `read-only`, `agent`, and `agent-full-access` accepted and reported back. Each wrote a file in the scratch workspace without a permission request. |
| Codex installed adapter inspection | `read-only` maps to workspace-write / on-request / user review; `agent` to workspace-write / on-request / auto review; `agent-full-access` to danger-full-access / never. The `read-only` ID does not mean approval before each tool. |
| OpenCode configuration | `plan` / `build` / `plan` accepted and reported back. The launch permission policy stayed fixed; this does not demonstrate changing that policy through ACP. |
| OpenCode model turn | The free service refused with `APIError`: “OpenCode's free tier can only be used from within OpenCode”. No successful turn or approval behaviour established. |
| Model selection on an existing idle session | Accepted and reported back by all three: Claude `default` / `sonnet` aliases; Codex `gpt-6-astra` / `gpt-5.6-sol`; OpenCode `opencode/big-pickle` / `opencode/ling-3.0-flash-fin-free`. |

Model selection acknowledgement does not establish a successful subsequent turn
on each model, exact catalog-ID support for Claude, restoration, or switching
while a turn runs. The spike did not test lost replies, audit/storage failures,
restart recovery, or the complete Nessa launch configuration. Its observations
are evidence for this proposal, not production regression coverage.

The local probe scripts and raw recordings are under
`/tmp/nessa-acp-231-spike/` on the spike host; that temporary directory is not a
repository dependency or durable test fixture. The results needed to assess the
proposal are recorded above.


## Follow-up: Claude and Codex behaviour

The [detailed investigation](../../reviews/231-acp-approval-spike.md) and its
[normalized recordings](../../reviews/evidence/231-acp-approval-macos.json)
extend the initial spike with workspace/outside reads and writes, harmless shell
execution, network escalation, a real test MCP server, mode downgrade, invalid
configuration, pending permissions, and fresh-process resume. These are observed
provider behaviours at the pinned versions, not completed Nessa integration tests.

- **Claude `acceptEdits` is automatic workspace editing.** With Nessa's deny list
  and no explicit ask rules, workspace reads and writes ran; outside reads and
  writes, MCP and WebFetch asked. Bypass ran those tested operations without
  asking. Returning to default restored the requests. Native Bash remained
  unavailable throughout. The complete behaviour matrix belongs to the report.
- **Ask rules survive bypass.** Both blanket and selective ask rules continued to
  request approval. `set_config_option permissions` is unsupported. Reopening
  the same Claude session in a fresh process with a changed rule set worked:
  adding the blanket rule made workspace Read ask; removing it and selecting
  `acceptEdits` allowed workspace Read/Write. Production reopening still needs
  Nessa's restoration and failure design.
- **Codex `agent` uses automatic review.** The harmless outside write and network
  escalation went through Guardian review without a user approval request.
  `read-only` allowed workspace writes, outside reads and harmless shell, but
  asked for the tested outside write, MCP tool and network escalation. Returning
  to `read-only` restored those requests. It is not Claude's `acceptEdits` policy.
- **Claude also has native `auto`.** It ran the benign test actions on Sonnet,
  but selecting Haiku changed the effective mode to `acceptEdits`; requesting
  `auto` on Haiku also returned that fallback. The decision below adopts native
  `auto` only where supported, with model-dependent availability and rejection
  of unintended fallback.
- **The harnesses do not enforce idle-only changes.** Both accepted a mode change
  while an approval was pending. Nessa must own the proposed `turnRunning`
  rejection. Claude emitted its expected mode notification before the RPC reply;
  Codex stored the selection for subsequent turns without a mode notification in
  the captured changes.

The later boundary probes also found that automatic review is not necessarily
an approval prompt: Claude's classifier denied an authorized fixture deletion
without asking, while Codex's Guardian approved that same disposable operation.
Both ignored the tested injected instructions before attempting the forbidden
actions. Preserve the distinction between agent refusal, classifier decision,
and user approval; “anything risky still asks” is not supported. The
[detailed report](../../reviews/231-acp-approval-spike.md#follow-up-deny-list-coverage-and-automatic-review-counterexamples)
records each deny-list entry, controls, decision evidence and remaining limits.

The policy and recovery decisions below are settled. Remaining work is to
implement and verify the Nessa lifecycle and cover consequential failures. Automatic-review
classification belongs to the ACP providers. Nessa exposes the supported modes
and preserves the outcomes they provide; reproducing or certifying arbitrary
Claude/Codex classifier decisions is outside this feature's scope and is not a
release gate. The observed counterexamples settle the UI wording, while Nessa
continues to own its explicit restrictions and mode-change lifecycle.

## Decision

### 1. The gateway publishes what can be chosen

A new product read, `agents.list`, returns the agents configured on this gateway.
For each agent it gives:

- `agent` (`claude` | `codex` | `opencode`);
- the models this gateway will run for it: catalog models whose provider
  matches the agent and whose launch profile is available (OpenCode's fixed
  profile currently offers only its configured model), as `{modelId,
  displayName, maxContextWindowTokens, reasoning, imageInput}`;
- `defaultModel`, the configured `agents.runtimes.<agent>.model`;
- each model's `approvalModes`, the subset of `ask` | `auto` | `full` this
  binding can honour and verify for that model, including binding-owned labels
  and descriptions (§3). There is no agent-wide list that implies every model
  supports Claude native `auto`.

The catalog stays where it is (`crates/nessa-sdk/data/models.json`). The panel
renders this list and holds no model or agent table of its own.

### 2. The model is fixed at creation

`ConversationCreateParams` gains an optional `model` (a catalog `modelId`)
alongside `agent`. The gateway resolves it:

| `agent` | `model` | Result |
| --- | --- | --- |
| absent | absent | chosen agent, its `defaultModel` |
| given | absent | that agent, its `defaultModel` |
| any | given, in that agent's list | that model |
| any | given, not in that agent's list | refused, typed `modelUnavailable` (no conversation is created) |
| — | — | an existing conversation ignores both, as `agent` is ignored today |

The resolved model is persisted on the conversation, and each session of that
conversation opens its binding with it:

- Claude: `_meta.claudeCode.options.model`
- Codex and OpenCode: `set_config_option model`

`verify_config` checks it as it checks the configured model today.
`ConversationRuntime.model` reports the conversation's model, not the
provider's. `ConversationRuntime` also gains:

- `agent`: the conversation's agent id, so the panel draws its mark from the id
  rather than parsing the harness name in `provider`;
- `modelName`: the catalog display name;
- `contextWindowTokens` and `reasoning`: the catalog's facts for that model.

A conversation's details can then name its model without the catalog read.

There is no mid-conversation model switch. Picking another model after the server
conversation exists opens a new draft, including when an image upload created it
before the first message. Carrying the transcript into that conversation
so the new agent continues from it is a separate, later decision.

### 3. Three approval modes, each mapped by its binding

The choices are native approval presets, with descriptions published by the
binding and rendered by the panel. `ask` does not promise approval before each
tool across agents. These mappings are the implementation decision; availability
still requires verification under Nessa's complete launch configuration. The
initial implementation focuses on Claude and Codex. OpenCode keeps its existing
fixed profile; new OpenCode mode switching and paid-provider probes are deferred.

| Mode | Claude | Codex | OpenCode |
| --- | --- | --- | --- |
| `ask` | `default` with native permission rules: reads may run; the provider requests approval where required | `read-only`: workspace actions may run; escalation asks the user | Current `plan` + restrictive launch policy; do not describe denied operations as approval requests |
| `auto` | Native `auto`, only on supported models: the provider's classifier reviews actions and may allow or deny without asking | `agent`: workspace actions may run; escalation uses native automatic review | Unavailable in this implementation |
| `full` | `bypassPermissions` without explicit ask rules: tested operations run; provider checks can still ask | `agent-full-access`: native full-access preset | Unavailable in this implementation |

- **The deny list remains policy.** Changing approval must preserve the binding's
  denied-tool boundary. The boundary follow-up accounts for all 17 entries:
  13 are offered without the policy and excluded with it across the tested modes;
  four are unavailable even in the baseline. Historical alias exclusion was
  also verified. That investigation exposed `ScheduleWakeup`, whose stop
  operation ran in bypass. It is now added to the same deny policy because
  scheduling work beyond the execution is outside Nessa's ownership. The list
  therefore contains 18 entries. “Every tool runs” is not the UI promise.
- **Claude uses one fixed native-rule profile.** Remove the blanket
  `permissions.ask: ["*"]` for conversations using this feature. Do not replace
  it with selective ask rules that also survive bypass. Preserve the explicit
  deny list and existing isolation of external settings/hooks in every mode.
  This intentionally replaces the current ask-before-every-tool/file-path
  contract: native `default` may read without asking. Label it “Provider asks”,
  not “Ask before every tool”. Returning to `ask` restores native `default`,
  not the old blanket rule. Normal changes use the mode RPC without reopening
  or changing permission rules. Restored sessions must start in this same
  profile; never reuse a process with the legacy blanket rule.
- **Native `auto` is not `acceptEdits`.** `acceptEdits` is not offered as the
  `auto` choice. The binding owns model-dependent availability, checked against
  the pinned provider's effective configuration. A success reply containing
  `acceptEdits` after requesting `auto` is a failed application, not success or
  an automatic substitution. Hide known-unsupported choices; reject newly
  discovered fallbacks and recover as in §4. Do not maintain a second model/mode
  capability table in the panel. Unknown support stays unavailable until the
  binding has verified it; classifier outcomes themselves are provider-owned.
- **Follow-up model verification (2026-09-27).** With the pinned Claude ACP
  0.76.0 and Codex ACP 1.12.0 adapters, changing the model and then each mode
  returned the selected effective mode for Claude Sonnet 5, Opus 5, Fable 5.1
  and Codex Astra, Sol, Terra, Luna. Claude Haiku 4.5 returned `acceptEdits`
  after `auto`. Isolated live native-edit
  turns succeeded on Opus 5 in `auto` and `bypassPermissions`, and Luna in
  `agent` and `agent-full-access`. These are partial probes, not the complete
  behavior matrix required below. They do not extend the binding-owned choices:
  only Sonnet and Astra retain Auto/Full; the other models retain Ask until
  the required behavior evidence exists.
- **Newly enabled presets require behaviour tests**, not
  just a configuration acknowledgement. Exercise reads, edits, shell, network,
  MCP and denied tools where the binding exposes them, using the complete Nessa
  launch configuration. Test transitions back to `ask` as well as elevation.
  The follow-up supplies direct-harness evidence for the named cases; production
  Nessa regression coverage and the report's excluded boundaries remain separate.
  Unsupported or unverified modes stay unavailable
  ([gate 7](../../../CODING_STANDARDS.md#gates)).
- **OpenCode is deferred.** Keep its existing fixed `plan` launch policy. Its
  models publish one fixed `ask` descriptor, labelled “Fixed plan policy”, whose
  description says operations may be denied rather than offered for approval.
  Creation with omitted mode or explicit `ask` uses that existing profile;
  `auto`/`full` are refused. A one-choice list renders no switch control. This
  represents the existing fixed configuration, not newly enabled switching or
  evidence of verified native approval behavior. OpenCode's paid
  permission round trip and policy replacement are separate follow-up work;
  they do not block Claude/Codex implementation.
- Unexpected `current_mode_update` remains a protocol error. Expected updates
  during an admitted change need ordering rules (§4); comparing them only to
  the old mode would reject the change being requested.

`agents.list` and the conversation view must publish the binding-owned
user-facing description for each offered mode alongside its ID, so the panel
neither invents a universal promise nor maintains its own provider policy table.

`ConversationCreateParams` gains an optional `approvalMode` (default `ask`). A
mode not in the selected model's binding-published `approvalModes` is refused, typed
`approvalModeUnavailable`. The conversation view gains `approvalMode` and
`approvalModes` (its binding's list for the conversation's fixed model from §1). The composer's tray can then offer
exactly what this conversation's agent honours without a second read.

### 4. Changing the mode mid-conversation

A new command, `conversation.setApprovalMode {conversationId, requestId, mode}`,
changes the choice. These are required implementation semantics, not claims
that the spike verified Nessa's lifecycle.

The conversation admission owner serializes mode changes, turn admission and
session close. The binding alone translates and checks the provider preset.
Claude and Codex use a live `session/set_config_option` mode RPC with the fixed
rules in §3. For Codex, “verified” means the pinned ACP adapter accepted and
reported the exact session mode; it does not certify every action the provider
will later take under that mode. Neither normal path requires reopening.
Unexpected configuration changes block
turn admission rather than silently updating the conversation's choice.

The durable conversation state owns the committed mode. A change has a correlated
record containing the request ID, caller, conversation, prior committed mode,
requested mode, and application/verification evidence. There are three admission
states: ready, changing, and recovery required. Changing and recovery required
both prohibit turn admission. The view reports the committed `approvalMode`
separately from change status and requested mode; an in-flight selection is never
shown as confirmed.

The order is explicit:

1. Validate authority, model/mode availability and idle state under the admission
   owner. Durably record intent before contacting the binding. Intent is not a
   committed mode. Failure here makes no provider call.
2. If a session is open, apply and verify the exact preset. If no session exists,
   record verification as deferred until open; do not start a process merely to
   change a saved choice.
3. Deliver the correlated outcome to the durable audit sink, then persist the
   committed mode and terminal request result together in one conversation
   storage operation. Audit evidence at this stage describes provider application
   (or deferred application), not an already committed conversation change. This
   avoids claiming a distributed transaction across audit and conversation stores.
   Only storage acknowledgement permits a success reply or reopening admission.
4. If the outcome or commit is uncertain, retain the intent and block turns.
   Reconcile by reading durable records, not by guessing from the RPC error.
   Recovery records link back to the same request and original application
   evidence; they do not rewrite it as a user refusal or confirmed rollback.

| State or event | Required result and regression |
| --- | --- |
| No live session, idle | Audit deferred application and commit the choice; verify it before the next session admits any turn. If open refuses or falls back, block and report `approvalModeNotApplied`; do not silently choose another mode. |
| Session open, idle | Record intent, apply exact preset, audit outcome, commit, then return success. Turn admission remains blocked throughout. |
| Turn running, including pending permission | Refuse `turnRunning`; no provider mutation. Record the refusal with caller and target. |
| Same mode, ready and verified (or no live session) | Return success without a provider mutation; retain a terminal idempotency result. An uncertain session cannot take this shortcut. |
| Repeated request ID, identical payload | Return its durable terminal result; while pending return `approvalModeUncertain` without reapplying, and expose the change status on read. A reused ID with different input is a typed request conflict. |
| Two changes, or a change racing a send/close | Serialize admission. A send admitted first causes `turnRunning`; a change admitted first blocks the send until resolved. A second change during recovery is refused `approvalModeUncertain`. Close may release resources but cannot erase pending intent/evidence. |
| Expected Claude notification precedes reply | Match the session generation and sole admitted change; retain it as evidence, but wait for exact reply verification and durable commit before success. |
| Provider reply, notification, or error does not prove the exact effective mode | Mark the effect uncertain, return `approvalModeUncertain`, retire the session, and recover the prior committed mode. A notification alone cannot commit a change. No Nessa-side classification of provider decisions is inferred. |
| Lost reply, timeout, or transport failure after dispatch | The effect is uncertain. Return `approvalModeUncertain`; admit no turns and recover the session. Do not retry the mutation on that session. |
| Intent persistence fails or its acknowledgement is lost | Do not call the provider. Read back by request ID before retrying; if the store cannot be read, remain blocked. |
| Audit fails after provider application | Do not commit the requested choice or return success. Report audit failure, retain application evidence, retire the session and recover the prior committed mode. Cleanup still runs if audit is unavailable; admission stays blocked until required evidence is durable. |
| Commit fails or acknowledgement is lost after audit | Read back the correlated terminal record. If committed, that mode/result is authoritative; if absent, the prior mode remains authoritative. If unreadable, remain blocked. Never infer rollback from a storage error. |
| Caller disconnects after admission | The owned operation continues to a durable terminal result or recovery state. Caller loss does not cancel evidence delivery, undo a provider effect, or release admission early. |
| Restart with unfinished intent | Start blocked; read the committed mode and terminal request record. Retire any surviving old session, reconcile the request, and reopen only from the authoritative committed choice. No blind replay of the requested change. |
| Session retirement or context restoration fails | Remain blocked with typed recovery failure. Preserve the conversation and transcript; do not replace it with an empty conversation or start a second process while the old one may still be live. |
| Restart after commit but before reply | Recover the committed mode and original terminal result; retry of the same request returns that result without another mode mutation. |

Recovery always retires the uncertain session and uses a fresh process with the
fixed profile, the same conversation/model, and the existing session restoration
contract. It restores the last durably committed mode, which may be the requested
mode if commit succeeded despite a lost acknowledgement. It does not attempt an
in-place rollback. The fresh attachment verifies the committed preset; recovery
does not send a second live mode mutation, which a fixed-mode provider cannot
perform. Verify the restored preset and record the recovery outcome
before permitting a turn; if either fails, remain blocked. When an unfinished
request has no commit, settle it as failed after recovery; a new user request
uses a new request ID. Recovery attribution is system-originated and retains the
original caller on the attempted change. Stale replies/notifications from a
retired session cannot resolve the new session's state.

This alpha release makes the new conversation schema the only current contract.
Every new conversation records a model and approval mode at creation. A local
database from the old schema is refused by the existing schema-version gate;
there is no migration, compatibility reader, or inferred model for old chats.
Users who retain an old alpha database must start with a fresh local database
to use this release. Release notes must also state that Claude's `ask` mode
uses native `default` behavior rather than the old blanket review rule.

### 5. Audit and implementation evidence

Every admitted change retains its intent, prior/requested/committed mode, caller,
provider acknowledgement and verification scope, terminal request result, and
any recovery cause/outcome. The durable audit records both the original request
time and each phase's observation time; an observation time does not claim when
an external provider effect occurred. Rejections retain typed reasons. Every
turn records the committed mode and session generation it was admitted under,
so an automatic action can be traced to its preset without inventing a user
approval.

The durable conversation record is authoritative for commitment; the audit sink
is authoritative for delivered application/recovery evidence. Correlation and
ordering are validated on restore. Audit delivery is independent of UI event
consumers and caller lifetime. Audit failure fails the command and blocks further
turns, while necessary cleanup proceeds. These requirements implement
[audit evidence](../../../CODING_STANDARDS.md#audit-evidence-is-part-of-the-behavior)
with the failure orderings in §4.

Implementation tests must cover the table with controlled failure seams, including
both outcomes of an ambiguous commit, audit failure during recovery, caller loss,
and send/change/close races. Binding integration tests cover the
complete fixed launch profile, exact model IDs, model-dependent `auto`, downgrade
to native `default`, and restoration without losing context. Claude and Codex are
the release scope. OpenCode expansion and arbitrary classifier certification are
not prerequisites. A stale Claude notification and fresh-process preset
reapplication on Claude and Codex have controlled ACP tests. Opt-in live tests
also exercise Nessa's Agent and saved-session restoration against pinned Claude
ACP (native `auto`) and Codex ACP (`ask` on `gpt-6-sol`). They do not certify
arbitrary classifier decisions or every live mode/model combination.

## Alternatives considered

- **Switch the model mid-session.** The spike observed idle model selection
  through ACP in all three harnesses. Nessa's current bindings still pin and
  validate their model; provider acknowledgement is not a complete product
  lifecycle. Keeping the model fixed is a product/scope decision that avoids
  live-switch, capability-refresh and restoration semantics in this change
  (gate 16). Transcript handoff remains a separate decision.
- **Keep the model table in the panel.** That would be two authorities over which
  models an agent runs (gate 13). The gateway already owns the catalog.
- **Apply a mode change at the next turn instead of refusing during a turn.**
  That adds a pending-mode state that must survive restarts and races with turn
  admission. Refusing is one sentence and has no hidden state.
- **One Nessa-level approval policy on top of the agents' own modes.** Nessa's
  policy hooks ([0014](0014-nessa-owned-policy-hooks.md)) may later tighten any
  mode. This record chooses the native approval preset and the binding permission rules needed to honour it.

## Consequences

- **Easier:** each conversation says what it runs on. The details sheet can show
  facts people care about. A new agent appears in the picker by being
  configured, with no panel change.
- **Harder:**
  - `auto` and `full` let agents act without asking, so the audit record is what
    remains. Each binding now carries a mode it must verify at open and on
    change, and a test per mode against its pinned version.
  - `models.json` becomes user-facing; a wrong display name is now visible.
- **Watch for:** an agent release that renames or drops a mode (its binding test
  fails and the mode leaves `approvalModes`). Also watch for people choosing
  `full` by default; that would argue for Nessa policy hooks sooner.
