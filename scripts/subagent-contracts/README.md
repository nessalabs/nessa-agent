# Subagent provider contract evidence

Developer tooling for [ADR 329](../../docs/adr/todo/329-subagents.md). A direct
local provider probe captured one native child lifecycle; cloud workers can
verify that contract without credentials, installed ACP harnesses or model calls.
This does not implement Nessa's proposed Agent parent relationship.

## Module map

| File | Owns |
| --- | --- |
| `capture.mjs` | Direct Codex ACP subprocess, empty temporary workspace, existing sign-in, bounded waits/retention, permission withdrawal and process-group cleanup |
| `evidence.mjs` | Allowlist extraction with identity replacement; checks agreement between the independently reported collaboration fields |
| `fixtures/codex-native.json` | Sanitized selected live frames, capture provenance, advertised modes and explicit result |
| `verify.mjs` | Credential-free inspection of that retained fixture; machine-readable JSON on stdout |
| `evidence.test.mjs` | Recorded success plus contradictory identity, state, ordering and redaction inputs |

`capture → evidence → fixture → verify/tests` means creation followed by reading;
there is no dependency from the product runtime to this tooling.

## Cloud verification

From the checkout root, using bare Node (no `node_modules`):

```sh
node scripts/subagent-contracts/verify.mjs
node --test scripts/subagent-contracts/*.test.mjs
```

The recorded native sequence is `spawnAgent → wait → closeAgent → end_turn`.
The checker compares `params.sessionId`, `rawInput.senderThreadId` and
`_meta.codex.collaboration.senderThreadId`; it also compares both receiver
lists and requires each `agentsStates` entry to describe a listed receiver.
A later wait/close cannot substitute a different child. The terminal result
cannot precede retained activity, and the probe's successful sequence requires
all three completed calls. Tests include the valid recording and contradictions
made by changing only one independently reported fact.

This is evidence inspection, not a replacement for ACP parsing or an executable
Nessa ownership model. Future native-child ingestion tests should consume the
retained JSON through their actual adapter; future Nessa parent cascade and
policy inheritance tests need controlled Agent/provider ports and their ADR
ordering cases. Those unimplemented guarantees have no fabricated live fixtures.

## Recorded run and meaning

The fixture records the run's exact UTC timestamp, platform, ACP package/source
hash, checked-in harness lock hash, capture-script hash, provider version, model,
client capabilities, selected mode and controlled prompt. The local run on
October 5, 2026 (America/Vancouver) used:

- `@agentclientprotocol/codex-acp@1.12.0`, `codex-cli 0.154.0`.
- `gpt-5.6-luna`, initial mode `read-only`; advertised presets had kinds
  `standard`, `auto_review` and `full_access`. No mode was broadened.
- One provider-native child, distinct from the parent. `spawnAgent` completed
  with child `pendingInit`; `wait` and `closeAgent` completed with child
  `completed` and controlled message `CHILD_DONE`; parent ended `end_turn`.
- Parent `session/close` returned an empty JSON-RPC result.
- Zero permission requests. This run provides no child approval/inheritance or
  denied-action evidence; the mode advertisement is not an inherited-policy proof.
- Zero `subagent_spawned`/`subagent_state_update` notifications despite declaring
  `clientCapabilities.subagents: {}`. The actual useful frames were ordinary
  `tool_call`/`tool_call_update` with `codex.collaboration` metadata. Advertising
  a capability does not establish which notification shape a run produces.

`closeAgent` reports a provider action and child state; `session/close` reports a
provider acknowledgement. Neither records physical descendant process termination,
recursive Nessa closure, durable restart recovery or cancellation of a running
child. The capture's subprocess group was stopped and reaped at the end of the
probe; that is local probe cleanup, not evidence for the proposed product cascade.
Direct native-provider success does not establish current Nessa support; this
probe bypasses its binding and changes no tool policy.

## Refreshing evidence locally

Only maintainers refreshing a provider contract need to run live sessions:

```sh
node scripts/subagent-contracts/capture.mjs /tmp/subagent-selected.json
```

The pinned ACP harness must already be installed and signed in. It starts the
adapter directly with an empty temporary workspace and no MCP servers. The prompt
asks for one child that returns a fixed string without tools, files, commands or
network. Any permission request is answered `cancelled`. Requests have 45-second
budgets (90 seconds for the prompt); raw retention is capped at 8 MiB and 10,000
frames. Errors retain typed attempt status instead of a successful recording.
A missing native sequence is an explicit failed attempt. The script terminates
its subprocess group, waits for exit and removes its workspace.

Raw frames exist only in memory and are discarded. The selector keeps only the
three native collaboration tool names and the terminal stop reason. It replaces
session/thread/tool identities consistently, removes delegated prompt text,
model/effort values and unknown metadata, and keeps only the controlled child
message sentinel. It drops auth/account updates, unrelated model text, filesystem
paths and unknown notifications. Original sender/receiver/agent-state relationships
remain checkable after replacement. Inspect selected JSON before replacing the
checked-in fixture; provenance/mode fields must contain only the built-in advertised
contract and controlled probe values. The raw adapter's stderr is drained and not
stored or printed.

Do not replace the captured absence of native session updates with synthesized
frames. A different provider/version or a capture with cancellation/reviews needs
its own truthful provenance and tests before becoming evidence for those cases.
