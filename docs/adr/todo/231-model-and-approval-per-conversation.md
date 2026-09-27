# 231. A conversation carries its own model and tool approval mode

## Purpose

People choose which model a conversation runs on, and how much its agent may do
without asking, from the composer. Today both are fixed for every conversation
of an agent. This record settles the wire contract, where each choice is owned,
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
- **Every binding is pinned to its most restrictive mode and refuses a change.**
  Claude runs `default` with the reviewed-tools ask rule and deny list, Codex
  runs `read-only`, and OpenCode runs `plan` (`*_acp/sessions/profile.rs`). A
  `current_mode_update` to anything else is a protocol error.
- **The client knows no catalog.** `models.json` is read only by the server, and
  the panel knows only the agent picked at setup.

The binding constraint is [gate 13](../../../CODING_STANDARDS.md#gates). Which
models an agent may run, and which approval modes it can honour, are each owned
once, by the gateway and the SDK binding. The panel reads them; it never retypes
them.

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

| Mode | Meaning shown | Claude | Codex | OpenCode |
| --- | --- | --- | --- | --- |
| `ask` | Nessa asks before each tool runs | `default` + ask rule (today) | `read-only` (today) | `plan` (today) |
| `auto` | Routine tools run; anything risky still asks | `acceptEdits` | `auto` | `build`, with `ask` for shell and network |
| `full` | Every tool runs without asking | `bypassPermissions` | `full-access` | `build`, with every permission `allow` |

- **The deny list stays in every mode.** `DISALLOWED_TOOLS` is policy, not
  approval.
- **A binding lists a mode in `approvalModes` only once it has a test** that the
  pinned agent version enters that mode and reports it back. A mode that cannot
  be verified against the pinned version is left out. The panel then shows it as
  not available for this agent rather than pretending
  ([gate 7](../../../CODING_STANDARDS.md#gates)).
- **The implementer confirms OpenCode's `build` permission rules** against the
  pinned release before listing `auto` or `full`.
- `current_mode_update` stays a protocol error, now against the conversation's
  mode rather than a constant.

`ConversationCreateParams` gains an optional `approvalMode` (default `ask`). A
mode not in the agent's `approvalModes` is refused, typed
`approvalModeUnavailable`. The conversation view gains `approvalMode` and
`approvalModes` (its agent's list from §1). The composer's tray can then offer
exactly what this conversation's agent honours without a second read.

### 4. Changing the mode mid-conversation

A new command, `conversation.setApprovalMode {conversationId, requestId, mode}`,
changes it. Gate 15's table:

| State when the change arrives | Result |
| --- | --- |
| No live session (idle, closed, or not yet created) | Accepted and persisted. The next session opens in the new mode. |
| Session open, no turn running | Accepted. The binding sets the mode with `set_config_option mode` and verifies the response before the command succeeds. On a refusal or a mismatch the old mode stays, and the command fails `approvalModeNotApplied`. |
| Turn running (including a pending permission) | Refused, `turnRunning`. The mode never changes under a running turn. The panel says to try again when the turn ends. |
| Same mode as current | Accepted without contacting the agent (idempotent). |
| Repeated `requestId` | Answered with the first result (the existing idempotent-request rule). |
| Two changes racing | Serialized by the conversation. Each is judged against the state the previous one left. |

A verified change that lands after a turn has already been refused is not
possible: refusal and acceptance are decided under the same conversation lock
that admits turns.

### 5. Audit

- **Every accepted mode change is recorded:** conversation, before and after,
  the caller, and whether the agent verified it.
- **Every turn records the mode it ran under.** A tool that ran without asking
  can then be traced to the mode that allowed it.
- **A refused change is recorded with its typed reason.**

These follow [audit evidence](../../../CODING_STANDARDS.md#audit-evidence-is-part-of-the-behavior).
An audit failure fails the command, but it does not leave the agent in a mode
the conversation does not record.

## Alternatives considered

- **Switch the model mid-session.** No binding supports it today, the capability
  (`modelSwitchReporting`) is unimplemented, and it adds a live-switch state
  machine. A new conversation is the simplest honest behaviour (gate 16), and
  the transcript handoff it leaves room for is what people actually want.
- **Keep the model table in the panel.** That would be two authorities over which
  models an agent runs (gate 13). The gateway already owns the catalog.
- **Apply a mode change at the next turn instead of refusing during a turn.**
  That adds a pending-mode state that must survive restarts and races with turn
  admission. Refusing is one sentence and has no hidden state.
- **One Nessa-level approval policy on top of the agents' own modes.** Nessa's
  policy hooks ([0014](0014-nessa-owned-policy-hooks.md)) may later tighten any
  mode. This record only chooses which native mode an agent runs in.

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
