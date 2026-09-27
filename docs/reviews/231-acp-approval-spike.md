# ADR 231: Claude and Codex approval evidence

Date: 2026-09-26. This is a direct-harness investigation for
[ADR 231](../adr/todo/231-model-and-approval-per-conversation.md), not a review
claim that the proposed Nessa feature is implemented.

## Environment and method

macOS, Claude ACP 0.76.0 / Claude Agent SDK 0.3.257, and Codex ACP 1.12.0 /
Codex 0.154.0. The Claude default alias resolved to Sonnet 5 in the observed
turns; Codex used `gpt-6-astra`, low effort. Harnesses came from the installed
packages in checkout `6a21ad7c7fc89b0381524ed1d32aa19bc29b0c81`. The
[normalized recordings](evidence/231-acp-approval-macos.json) retain request
results, permission offers, tool outcomes, file checks, and source/raw-log hashes.

The probes spoke JSON-RPC over stdio and used existing local authentication.
Child environments were narrowed to identity/path/locale/auth variables; Codex
started with `INITIAL_AGENT_MODE=read-only`. Claude disabled settings sources,
hooks and connectors, and received Nessa's current `DISALLOWED_TOOLS` both as
`disallowedTools` and permission denies. A trusted test MCP server exposed only
`record_probe`, appending a marker to its own scratch log. No persistent approval
was granted. The main probes cancelled permission requests; the separate tools
probe selected the offered `reject_once` and permitted only ToolSearch schema
loading when needed. Cancellation can terminate a Codex turn: actions after it
are not counted as attempted unless tool frames demonstrate the attempt.

Workspaces were under `/tmp/nessa-acp-231-deep`. Outside-path probes used newly
created directories under the user's home, outside `/tmp` and the workspace.
Network probes used `https://example.com`. Successful file writes were checked
on disk, MCP execution against its marker log, and shell/read/network results
against tool frames, not only the assistant's narrative. Processes were stopped
after the probes. This did not change the user's global agent configuration.

## Claude: native presets with Nessa's deny list and no explicit ask rules

“Asked” means an ACP permission request was observed and denied by the probe.
“Ran” means the tool completed without such a request for this input.

| Action | `default` | `acceptEdits` | `bypassPermissions` | Return to `default` |
| --- | --- | --- | --- | --- |
| Read workspace canary | Ran | Ran | Ran | Ran |
| Write workspace file | Asked; absent | Ran; present | Ran; present | Asked; absent |
| Read outside canary | Asked | Asked | Ran | Asked |
| Write outside file | Asked; absent | Asked; absent | Ran; present | Asked; absent |
| Test MCP marker | Asked; no marker | Asked; no marker | Ran; marker recorded | Asked; no new marker |
| WebFetch example.com | Asked | Asked | Ran; Example Domain returned | Asked |
| Native Bash | Unavailable | Unavailable | Unavailable | Unavailable |

Bash was excluded by the launch policy; this is not evidence that arbitrary
native shell commands become harmless in any mode. Nessa's real Shepherd MCP
shell was not exercised. The MCP result above establishes a custom MCP tool's
approval routing, not Shepherd's execution and audit lifecycle.

`acceptEdits` therefore has a concrete, narrower description: automatic workspace
edits, with approval still observed for the tested outside paths, MCP and fetch.
It is not equivalent to Codex's automatic reviewer.

## Claude: explicit rules and true automatic review

With the current blanket `ask: ["*"]`, workspace/outside reads and writes and
ToolSearch asked in `default`, `acceptEdits`, and `bypassPermissions`, including
return to default. Denied ToolSearch prevented MCP/WebFetch schema loading in
that run, so those downstream executions were not independently exercised there.

A second fixed profile asked for `Read`, `WebFetch`, `WebSearch`, and
`mcp__spike__*`, while retaining the deny list. Workspace writes ran in
`acceptEdits`; workspace and outside writes ran in `bypassPermissions`. Read,
MCP and WebFetch still asked in bypass. WebSearch was configured but not invoked.
Thus selective ask rules also survive mode changes; narrowing the wildcard does
not make a later bypass clear those rules.

The installed adapter's `canUseTool` explicitly forwards callbacks even when the
session advertises bypass: a callback can represent an explicit ask rule or a
provider check requiring interaction. Its bypass mode is not a guarantee that
no approval UI can appear.

Claude also exposes native `auto`, separate from `acceptEdits`. On the tested
Sonnet model, `auto` was accepted and the benign workspace/outside reads and
writes, MCP marker and WebFetch ran without an ACP approval. Returning to
`default` restored the tested permission requests. This does not characterize
what its reviewer decides for risky inputs.

A configuration-only counterexample matters: after selecting `auto` on Sonnet,
selecting `haiku` changed the effective mode to `acceptEdits`. Requesting `auto`
on Haiku then returned success with `acceptEdits` in `configOptions`. The pinned
`SessionModeManager` implements this model-dependent fallback. If the product
chooses true automatic review, availability must account for the model and the
binding must validate the effective mode, not merely a successful RPC. ADR 231's
existing `auto` → `acceptEdits` mapping remains a separate product choice.

## Codex: native presets

| Action | `read-only` | `agent` | `agent-full-access` | Return to `read-only` |
| --- | --- | --- | --- | --- |
| Read workspace and outside canaries with `cat` | Ran | Ran | Ran | Ran |
| Write workspace file | Ran | Ran | Ran | Ran |
| Write outside file | Asked; absent | Guardian review; written without user request | Written without user request | Asked; absent |
| Test MCP marker | Asked; no marker | Ran; marker recorded | Ran; marker recorded | Asked; no new marker |
| Harmless `printf` command | Ran | Ran | Ran | Ran |
| `curl` HEAD example.com | Initial DNS failure; escalation asked and was denied | Initial DNS failure; automatic review allowed escalation, HTTP 200 | HTTP 200 without escalation | Initial DNS failure; escalation asked and was denied |

The installed `AgentMode` definitions explain these results:

| ACP ID | Sandbox | Approval policy | Reviewer |
| --- | --- | --- | --- |
| `read-only` | `workspaceWrite` | `on-request` | `user` |
| `agent` | `workspaceWrite` | `on-request` | `auto_review` |
| `agent-full-access` | `dangerFullAccess` | `never` | `user` (policy does not request approval) |

The first two also set network access false and do not exclude `/tmp` or TMPDIR
from writable sandbox roots. “Workspace only” is consequently too strong a
filesystem description. Reads outside the workspace are not automatically
approval-gated. Nessa does not apply Claude's native-tool deny list to Codex;
Codex's native shell remains available.

Codex MCP approvals may carry only the correlated `toolCallId`, kind and pending
status, with `_meta.is_mcp_tool_approval`; the observed permission had no title or
raw arguments. Consumers must use the preceding tool observation, not infer an
identity from a missing display field. Persistent options were offered by the
provider but not selected in these tests.

## Configuration, notifications, active turns and restoration

| Case | Observed result / source finding | Consequence for Nessa |
| --- | --- | --- |
| Claude idle mode change | `current_mode_update` arrived before the successful `set_config_option` response in the recorded sequence. `selectMode` awaits SDK `setPermissionMode`, then the ACP handler publishes and updates its state. | Admit expected notifications during a requested change; comparing only with the old mode would reject it. |
| Codex idle mode change | Successful returned config; no mode notification in the captured changes. `applyModeChange` changes adapter session state; `sendPrompt` passes policy, reviewer and sandbox to `runTurn`. | The acknowledgement means a preset selected for subsequent turns, not an immediate provider permission handshake. |
| Change while an approval is pending | Both accepted the full/bypass selection before the outstanding permission was answered. The probe then cancelled the pending request; neither target file was created. | Nessa must enforce its own `turnRunning` refusal; neither harness enforces that product rule. This did not test later tools in a surviving active turn. |
| Invalid mode | Claude returned `-32603` with an invalid-value diagnostic; Codex returned `-32602`. | Map RPC failure without branching on diagnostic text. |
| `set_config_option` with `configId=permissions` | Claude: unknown option (`-32603`); Codex: invalid params (`-32602`). Source dispatch provides no general permission-rule setter through this method. | Mode selection is not a live ask-rule replacement operation. |
| Claude fresh-process resume with blanket rule added | Same session ID resumed; workspace Read asked. | Resume can apply changed launch rules in this tested case. |
| Claude another fresh-process resume with blanket removed | Same session ID resumed; selecting `acceptEdits` allowed workspace Read and Write without asking. | Reopening is a viable mechanism to investigate for mode-specific rule sets; no live rule mutation was demonstrated. |
| Codex fresh-process resume | Same session ID resumed in the requested `read-only` preset; workspace edit succeeded without approval. | Resume worked for this basic case; persist/reapply the product's chosen preset rather than infer it from prior UI state. |

The Claude resume checks reused an existing transcript but did not verify full
transcript equivalence, hidden provider state, or Nessa's restoration machinery.
Neither source inspection nor these successful calls establishes atomicity across
provider effects, Nessa persistence and audit. Lost replies and crashes still
need the ADR's failure design and deterministic integration tests.

## Implementation conclusions and remaining boundaries

The unknown is no longer whether ordinary Claude/Codex tool classes ask in the
candidate profiles: the matrices above record what was observed. What remains
is a product decision and lifecycle implementation:

- Preserving Claude's strict ask-for-tools profile while offering automatic edits
  or bypass requires changing the ask rules as well as the mode. Fresh-process
  resume worked for adding/removing rules; a plain live mode RPC does not do it.
- A fixed native-rule profile permits live `default` / `acceptEdits` / bypass
  selection, but changes today's strict read-approval semantics. That change
  needs an explicit decision, including the file-by-path approval contract.
- True Claude `auto` would better match the concept of automatic review, but
  adds model-dependent availability and fallback handling. The spike does not
  silently substitute it for the ADR's `acceptEdits` choice.
- Approval descriptions must distinguish automatic workspace edits from automatic
  review, and preserve provider checks in bypass. A deterministic list of what
  a model-based reviewer will approve is not available from these probes.

Excluded: production Nessa end-to-end execution, audit/store failures, caller
loss, crash recovery, the full denied-tool list (only native Bash exclusion was
actively checked), WebSearch, sensitive OS paths, symlinks, arbitrary shell
commands, subagents, scheduled work, persistent grants, enterprise/managed
policies, other models/OSes and classifier rejection boundaries. No result here
claims universal safety or that all operations in a tool category share the
same decision.

## Reproduction and evidence ownership

Raw JSONL, scratch MCP server and Python drivers remain locally in
`/tmp/nessa-acp-231-deep`; the earlier quick spike remains in
`/tmp/nessa-acp-231-spike`. These are temporary investigation artifacts, not a CI
suite. Their paths are not required to read the normalized evidence committed
alongside this report. To repeat the experiment, install the pinned harnesses,
initialize ACP, create a scratch session with the stated rules and test MCP
server, select each mode, run the named canary actions, deny requests, inspect
both tool frames and effects, then return to the initial mode. For restoration,
stop the process and resume the same ID with the changed launch settings.

Source inspection covered the installed Claude `acp-agent.js` and
`session-mode.js` (`canUseTool`, `setSessionConfigOption`, `selectMode`,
`reconcileForModel`, `createSession`, `resumeSession`) and Codex `index.js`
(`AgentMode`, `applyModeChange`, `sendPrompt`, `CodexApprovalHandler`,
`buildMcpPermissionRequest`). The evidence file records their hashes. These are
provider implementation observations at the stated pins, not duplicated Nessa
policy implementations.
