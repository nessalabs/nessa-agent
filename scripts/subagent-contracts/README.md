# Subagent provider contract evidence

Developer tooling for [ADR 329](../../docs/adr/todo/329-subagents.md). A direct
local provider probe captured one native child lifecycle; cloud workers can
verify that contract without credentials, installed ACP harnesses or model calls.
This does not implement Nessa's proposed Agent parent relationship.

## Module map

| File | Owns |
| --- | --- |
| `capture.mjs` | Direct provider probe orchestration, source hashes, controlled-result selection and final attempt output |
| `acp-session.mjs` | Pre-parse byte framing, ACP envelope checks, request deadlines, permission withdrawal and supervised callback failure |
| `processes.mjs` | POSIX process-group cleanup independent of leader exit; bounded provider-version subprocess |
| `evidence.mjs` | Allowlist extraction with identity replacement; checks agreement between the independently reported collaboration fields |
| `fixtures/codex-native.json` | Sanitized selected live frames, capture provenance, advertised modes and explicit result |
| `verify.mjs` | Credential-free inspection of that retained fixture; machine-readable JSON on stdout |
| `evidence.test.mjs` | Recorded success plus contradictory identity, state, ordering and redaction inputs |
| `capture.test.mjs`, `fixtures/probe-agent.mjs` | Explicit scripted failure data and real subprocess cleanup regressions; not provider recordings |

`capture → acp-session → processes` owns local subprocess work;
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
A later wait/close cannot substitute a different child. A call's completion must also retain the sender/receiver target admitted at its
start; spawn alone may acquire a previously unknown child identity on completion.
The terminal result cannot precede retained activity. The controlled successful
sequence requires all three completed calls, `end_turn`, and `completed` plus
`CHILD_DONE` reports from wait/close. A cancelled terminal, errored child or wrong
child response yields a typed unsuccessful attempt. Tests include the valid recording and contradictions
made by changing only one independently reported fact.

This is evidence inspection, not a replacement for ACP parsing or an executable
Nessa ownership model. Future native-child ingestion tests should consume the
retained JSON through their actual adapter; future Nessa parent cascade and
policy inheritance tests need controlled Agent/provider ports and their ADR
ordering cases. Those unimplemented guarantees have no fabricated live fixtures.

## Recorded run and meaning

The fixture records the run's exact UTC timestamp, platform, ACP package/source
hash, checked-in harness lock hash, capture/session/process/selector source hashes, provider version, model,
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
child. The refreshed fixture separately records local probe cleanup: the leader was
reaped and no live members remained in its POSIX process group. The group stays
owned after leader exit, through escalation and confirmation. Orphan zombies are
not counted as running processes; a process that escapes into another group is
outside this fixture's ownership. This is local probe cleanup, not evidence for
the proposed product cascade.
Direct native-provider success does not establish current Nessa support; this
probe bypasses its binding and changes no tool policy.

## Capture lifecycle and ordering cases

This table is the design for the developer probe, not for Nessa Agent ownership.
The capture owner retains the subprocess group independently from the leader's
stdio and exit. A controlled successful recording requires `end_turn` plus the
completed `CHILD_DONE` child reports; advertised capability or a completed tool
call alone is insufficient.

| State/event or ordering | Decision/effect | Regression |
| --- | --- | --- |
| Starting; setup or spawn failure | Retain a typed attempt failure, close any acquired process group, remove the temporary workspace | Setup/spawn failure test |
| Reading; null, array, malformed or invalid ACP envelope | Reject outstanding requests with `invalid_frame`, stop reading, then use the common cleanup owner; retain no raw text | Malformed-envelope subprocess test |
| Reading; terminated or unterminated line exceeds the remaining byte budget | Refuse before concatenating/decoding/parsing the line, fail outstanding waits with `recording_limit`, then close the group | Oversized-line subprocess tests |
| Reading; native wait/close starts | Retain sender/receiver target with the call identity; only its matching completion may settle it | Foreign wait/close start plus original-child completion tests |
| Reading; spawn starts without a receiver | Permit the provider to supply the child's identity on completion | Recorded spawn counterpart |
| Settling; cancelled terminal or errored/non-sentinel child outcome | Keep a typed unsuccessful probe outcome, never report controlled success | Cancelled/error/wrong-result tests |
| Settling; controlled sequence completes | Inspect correlated evidence and perform bounded provider-version lookup | Recorded controlled success |
| Version lookup stalls or overflows output | Terminate its separately owned process group, report version unavailable, continue ACP cleanup | Stalled version binary test |
| Closing; leader exits before its SIGTERM-ignoring child | Retain group ownership through SIGKILL escalation and process-state confirmation | Leader-first real subprocess test |
| Closing; leader reaped and no live group members | Record stopped cleanup and remove the workspace; zombies are not live members, descendants outside the group are outside this fixture's ownership | Cleanup success assertions |
| Closing; termination or process-state confirmation fails | Retain typed `cleanup_unconfirmed`, do not present a successful probe | Failure outcomes and documented limit |

## Refreshing evidence locally

Only maintainers refreshing a provider contract need to run live sessions:

```sh
node scripts/subagent-contracts/capture.mjs /tmp/subagent-selected.json
```

Live capture requires POSIX process-group ownership and `/bin/ps` state queries;
Windows live capture is explicitly refused, while credential-free evidence
inspection and identity tests remain portable. The pinned ACP harness must
already be installed and signed in. It starts the
adapter directly with an empty temporary workspace and no MCP servers. The prompt
asks for one child that returns a fixed string without tools, files, commands or
network. Any permission request is answered `cancelled`. Requests have 45-second
budgets (90 seconds for the prompt). Incoming wire bytes, including an
unterminated line, are capped at 8 MiB before decoding/JSON parsing; retained
serialized frames share an 8 MiB bound and a 10,000-frame cap. This bounds retained
wire data rather than claiming an 8 MiB ceiling for parsed-object/transient memory.
The provider-version subprocess has a two-second budget and 4 KiB output bound;
a stalled or failed lookup is reported as unavailable after its own cleanup. Errors retain typed attempt status instead of a successful recording.
A missing native sequence is an explicit failed attempt. The script retains group ownership for a two-second TERM grace period, then
escalates to KILL and allows two seconds for confirmation. Bounded process-state
queries cannot postpone that phase's deadline. Success requires a reaped leader
and no live group members. Unconfirmed cleanup remains typed failure; its
workspace is retained because external work may remain. Confirmed cleanup removes
the workspace. Setup metadata is read before spawning, and malformed-frame/callback
failures use this same cleanup owner.

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
