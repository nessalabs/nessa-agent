# 231. A conversation carries its own model and tool approval mode

## Purpose

People choose which model a conversation runs on, and how much its agent may do
without asking, from the composer. Today both are fixed for every conversation
of an agent. This record proposes the wire contract, where each choice is owned,
how the three approval modes map onto each agent's own modes, and what happens
when the mode changes mid-conversation.

- **Date:** 2026-09-26
- **Status:** proposed
- **Issue:** [#231](https://github.com/nessalabs/nessa-agent/issues/231)

## Context

- **One model per agent.** `agents.runtimes.<agent>.model` builds one provider
  per agent (`composition/agent.rs`, `build::provider`), with the model baked into
  it. Sessions already run one process each (`acp/sessions/binding.rs`), so a
  model per session costs no new process model. It only has to reach the binding.
- **Creation is lazy.** The panel opens a tab locally and sends
  `conversation.create` on the first send (`gateway/effects.ts`). So choosing a
  model before that is free: nothing exists yet to change.
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
  `auto` on Haiku also returned that fallback. This is a possible alternative
  to the proposed mapping, not an adopted substitution. Adopting it requires
  model-dependent availability and rejection of unintended fallback.
- **The harnesses do not enforce idle-only changes.** Both accepted a mode change
  while an approval was pending. Nessa must own the proposed `turnRunning`
  rejection. Claude emitted its expected mode notification before the RPC reply;
  Codex stored the selection for subsequent turns without a mode notification in
  the captured changes.

The remaining work is explicit: select Claude's rule strategy, implement and
verify the Nessa lifecycle, cover consequential failures, and test additional
policy boundaries listed in the report. The finite benign probes cannot define
what a provider's automatic reviewer will decide for arbitrary risky input.

## Decision

### 1. The gateway publishes what can be chosen

A new product read, `agents.list`, returns the agents configured on this gateway.
For each agent it gives:

- `agent` (`claude` | `codex` | `opencode`);
- the models this gateway will run for it: every catalog model whose provider
  matches the agent, as `{modelId, displayName, maxContextWindowTokens,
  reasoning, imageInput}`;
- `defaultModel`, the configured `agents.runtimes.<agent>.model`;
- `approvalModes`, the subset of `ask` | `auto` | `full` this binding can honour
  and verify (§3).

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

There is no mid-conversation model switch. Picking another model after the first
message opens a new conversation. Carrying the transcript into that conversation
so the new agent continues from it is a separate, later decision.

### 3. Three approval modes, each mapped by its binding

The choices are native approval presets, with descriptions published by the
binding and rendered by the panel. `ask` does not promise approval before each
tool across agents. The proposed mappings below are candidates until behaviour
is verified under Nessa's complete launch configuration.

| Mode | Claude | Codex | OpenCode |
| --- | --- | --- | --- |
| `ask` | `default` + blanket ask rule: ask before tools covered by that rule | `read-only`: workspace actions may run; escalation asks the user | Current `plan` + restrictive launch policy; do not describe denied operations as approval requests |
| `auto` | `acceptEdits` without the blanket ask rule: workspace edits run; tested outside paths, MCP and WebFetch ask (see follow-up) | `agent`: workspace actions may run; escalation uses native automatic review | Candidate `build` with explicit permission rules; unavailable until verified |
| `full` | `bypassPermissions` without explicit ask rules: tested operations run; provider checks can still ask | `agent-full-access`: native full-access preset | Candidate `build` with explicit allow rules for permitted tools; unavailable until verified |

- **The deny list remains policy.** Changing approval must preserve the binding's
  denied-tool boundary. Native Bash exclusion was checked through elevated modes;
  the remaining deny-list entries still need coverage before offering them.
  “Every tool runs” is not the UI promise.
- **Claude's mode and permission rules must be designed together.** Retaining
  `ask: ["*"]` defeated automatic file approval in the spike, including in
  `bypassPermissions`. Removing or replacing that rule, and restoring it when
  returning to `ask`, requires a rule-strategy decision. Fresh-process resume
  applied changed rules in the follow-up; live `set_config_option permissions`
  was refused. Preserve strict ask semantics through verified reopening, or
  explicitly decide to adopt a fixed native-rule profile with different read
  approval semantics. Neither choice is silently made by this spike.
- **A binding lists a mode in `approvalModes` only after behaviour tests**, not
  just a configuration acknowledgement. Exercise reads, edits, shell, network,
  MCP and denied tools where the binding exposes them, using the complete Nessa
  launch configuration. Test transitions back to `ask` as well as elevation.
  The follow-up supplies direct-harness evidence for the named cases; production
  Nessa regression coverage and the report's excluded boundaries remain separate.
  Unsupported or unverified modes stay unavailable
  ([gate 7](../../../CODING_STANDARDS.md#gates)).
- **OpenCode mode and launch permission policy are separate.** Switching to
  `build` does not establish the proposed `auto` or `full` policy. Verify the
  paid-provider permission round trip and how policy changes reach a live or
  reopened session before offering those modes.
- Unexpected `current_mode_update` remains a protocol error. Expected updates
  during an admitted change need ordering rules (§4); comparing them only to
  the old mode would reject the change being requested.

`agents.list` and the conversation view must publish the binding-owned
user-facing description for each offered mode alongside its ID, so the panel
neither invents a universal promise nor maintains its own provider policy table.

`ConversationCreateParams` gains an optional `approvalMode` (default `ask`). A
mode not in the agent's `approvalModes` is refused, typed
`approvalModeUnavailable`. The conversation view gains `approvalMode` and
`approvalModes` (its agent's list from §1). The composer's tray can then offer
exactly what this conversation's agent honours without a second read.

### 4. Changing the mode mid-conversation

A new command, `conversation.setApprovalMode {conversationId, requestId, mode}`,
changes it. This remains a proposed lifecycle, not a capability established by
the spike. Mode selection alone is insufficient when a binding also needs to
change launch permission rules. Before implementation, settle whether each
binding can apply the complete policy live or needs session reopening, including
how reopening preserves the conversation context. Claude rule replacement worked
through fresh-process resume in the follow-up; Codex applies its selected preset
to subsequent turns. Gate 15's initial table:

| State when the change arrives | Result |
| --- | --- |
| No live session (idle, closed, or not yet created) | Accepted and persisted. The next session opens in the new mode. |
| Session open, no turn running | Accepted. The binding applies and verifies the complete mode and permission policy before the command succeeds; the per-binding mechanism remains to be settled. A confirmed refusal with unchanged policy fails `approvalModeNotApplied`. A mismatch or lost reply does not prove that the old policy remains; recovery is unresolved below. |
| Turn running (including a pending permission) | Refused, `turnRunning`. The mode never changes under a running turn. The panel says to try again when the turn ends. |
| Same mode as current, with verified effective policy | Accepted without contacting the agent (idempotent). An uncertain earlier change must first be reconciled. |
| Repeated `requestId` | Answered with the first result (the existing idempotent-request rule). |
| Two changes racing | Serialized by the conversation. Each is judged against the state the previous one left. |
| Expected Claude mode notification before the command reply | Correlate with the admitted change; do not reject merely because it differs from the previous mode. Observed by the follow-up. |
| Successful RPC reports a different effective mode | Do not publish the requested mode as applied. Observed for Claude `auto` on Haiku; model-dependent modes require explicit availability and fallback handling. |

The proposed admission owner must serialize mode changes with turn admission.
Before implementing this command, extend the table with lost replies after application, partial mode/rule
changes, audit or storage failure after application, caller disconnect, and
restart recovery. Define how further turns are blocked while effective policy
is uncertain and how it becomes verified again. The spike supplies no rollback
guarantee; this proposal remains open on those failure orderings.

### 5. Audit

- **Every accepted mode change is recorded:** conversation, before and after,
  the caller, and whether the agent verified it.
- **Every turn records the mode it ran under.** A tool that ran without asking
  can then be traced to the mode that allowed it.
- **A refused change is recorded with its typed reason.**

These follow [audit evidence](../../../CODING_STANDARDS.md#audit-evidence-is-part-of-the-behavior).
An audit failure must fail the command. How a provider policy already changed
is reconciled with durable conversation state remains part of §4's unresolved
failure design; reporting failure alone does not restore the previous policy.

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
