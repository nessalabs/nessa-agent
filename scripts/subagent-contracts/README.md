# Subagent provider contract evidence

Developer tooling for [ADR 329](../../docs/adr/todo/329-subagents.md). A direct
local provider probe captured one native child lifecycle; cloud workers can
verify that contract without credentials, installed ACP harnesses or model calls.
This does not implement Nessa's proposed Agent parent relationship.

## Module map

| File | Owns |
| --- | --- |
| `capture.mjs` | Direct provider probe orchestration, manual SIGINT/SIGTERM supervision, source hashes, controlled-result selection and final attempt output |
| `acp-session.mjs` | Sequential RPC/opening authority, pre-parse framing, first-failure retention and immutable record sealing |
| `processes.mjs` | POSIX process-group cleanup independent of leader exit; bounded provider-version subprocess |
| `metadata.mjs` | Installed-adapter identity match, capability presence and closed mode/kind projection; provider text and nested metadata are omitted |
| `evidence.mjs` | Sealed-only projection; normalize opening/prompt boundary with tool IDs and check their agreement |
| `fixtures/codex-native.json` | Sanitized selected live frames, capture provenance, advertised modes and explicit result |
| `verify.mjs` | Credential-free inspection of that retained fixture; machine-readable JSON on stdout |
| `provenance.test.mjs` | Real Git checkouts prove hashed source bytes remain LF with autocrlf enabled or disabled |
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
The independently retained `admission` record names the matching opening RPC,
its admitted session, the own prompt request/response and the own close
request/acknowledgement. These facts are validated before normalization; native
updates cannot supply their own parent or terminal authority. Cloud inspection
checks the normalized frames against that separate opening/prompt boundary.
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
October 6, 2026 (America/Vancouver) used:

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
Cleanup confirmation requires the capture process to reach its cleanup owner.
Catchable manual `SIGINT` and `SIGTERM` record one interrupted attempt, then use
that same owner. The process writes the attempt and exits only after the owner
reports confirmed group release or explicit unconfirmed workspace ownership. A
later catchable signal does not replace the first cause. Parent crash and
`SIGKILL` do not enter the handler: the detached provider group and any workspace
are then the supervisor's to reap. That boundary is
[#591](https://github.com/nessalabs/nessa-agent/issues/591).

## Capture lifecycle and ordering cases

The earlier collector inferred admission and terminal authority from provider
updates. The probe now owns one sequential RPC boundary: opening supplies the
admitted session, its own prompt request supplies terminal correlation, and its
matching close response permits sealing. A sealed immutable snapshot is the sole
input to sanitization; cleanup and later version metadata cannot change its facts.
Sealing closes record ingress. It proves neither that an ACP session physically
terminated nor that a remote child stopped. A matching close RPC error is a typed
failed probe under this controlled-success contract, preserved independently from
physical cleanup.
This is a structural repair of the developer probe, not Nessa Agent ownership.

| State/event or ordering | Owner decision/effect | Regression |
| --- | --- | --- |
| Starting; initialize/new dispatched | Retain the one active RPC identity and method; unmatched responses fail `unsolicited_response` | Unsolicited terminal subprocess test |
| Opening; native collaboration arrives before an admitted prompt | Fail `premature_activity`; updates cannot invent admission | Startup activity subprocess test |
| Initialize/new metadata | Match installed-adapter identity, project capability presence and admitted mode/kind values; omit provider labels, paths and unknown content before serialization | Secret-bearing metadata and rejected identity/mode subprocess tests |
| Opening; matching new response | Retain its session ID as independent opening evidence; enter Open | Valid recorded opening |
| Open; own prompt dispatched | Bind its request ID and admitted session; enter Prompt active | Valid opening/prompt boundary checks |
| Prompt active; native tool update | Validate the raw envelope/sender against the admitted session before normalization; retain the call's sender/receiver target | Foreign session and wait/close target tests |
| Prompt active; unsolicited terminal response | Fail `unsolicited_response`; it cannot supply prompt completion | Forged terminal plus empty own prompt response test |
| Prompt active; matching own prompt response | Retain that exact response and stop reason; enter Correlated terminal; empty result fails `prompt_invalid` | Valid/cancelled/empty prompt tests |
| Correlated terminal; native activity | Fail `activity_after_terminal` | Premature/trailing activity neighbors |
| Correlated terminal; own close dispatched | Bind the close request to the admitted session; enter Closing | Matching close evidence |
| Closing; matching close response | Retain response; enter Close answered; rejected/invalid close stays failed | Failed close subprocess test |
| Close answered; later invalid frame in the same chunk or unfinished line at sealing | Preserve the first recording failure; no sealed success; physical cleanup still runs | Close then null/partial-line subprocess tests |
| Close answered; seal after current decoding finishes | Validate no failure/pending line, seal admission plus recording as immutable serialized evidence; stop recording | Valid chunk, boundary replay and sealing tests |
| Sealed; slow version lookup or later provider output | Version may add bounded metadata; raw output is discarded and cannot reopen or invalidate the sealed recording | Late-output/stalled-version subprocess test |
| Failed; cleanup confirms release | Retain the primary recording failure alongside cleanup; confirmed cleanup cannot turn failure into success | Close/null failure plus confirmed cleanup |
| Failed/Sealed; cleanup unconfirmed | Retain cleanup failure independently and retain workspace ownership | Failed-close/unconfirmed-cleanup distinction |
| Reading; stdout EOF with unterminated JSON | Refuse the buffered fragment through the same completion rule; EOF cannot admit a response without its newline | Split JSON EOF and final-close subprocess regressions |
| Reading; invalid/null envelope or handler fault | Supervise first failure, reject active RPC, stop reading and start common cleanup | Malformed-envelope tests |
| Reading; terminated/unterminated line exceeds wire bound | Refuse before decoding/parsing, retain `recording_limit`, close group | Oversized-line tests |
| Inspecting sealed evidence; cancelled/errored/non-sentinel outcome | Typed unsuccessful probe; success requires `end_turn` and completed `CHILD_DONE` wait/close reports | Controlled-outcome tests |
| Version process stalls/overflows | Bound and stop its separate group; record version unavailable | Stalled-version test |
| Closing group; leader exits first | Retain group authority through SIGKILL and live-member confirmation | Leader-first real-process test |
| Manual command; no signal; scripted success or recording failure | Handlers stay idle; completed and failed attempts keep their causes, cleanup, and exit status | Manual-entry complete and invalid-json |
| Manual command; SIGINT/SIGTERM before `session/new` is admitted | Record `interrupted` and that signal as the attempt cause; do not prompt; the shared cleanup owner confirms release or records unconfirmed ownership; delete the workspace only after confirmed stop; write the attempt and exit after that settlement | Manual-entry hold-initialize SIGINT |
| Manual command; SIGINT/SIGTERM during a pending provider RPC | Same interrupted cause; a later SIGINT/SIGTERM does not replace the first signal; cleanup evidence stays independent of the attempt cause; unconfirmed cleanup keeps that cause and the workspace | Manual-entry hold-prompt, SIGINT then SIGTERM during group grace; grace/confirm 0 retains the workspace |
| Manual command; SIGINT/SIGTERM while cleanup is settling an earlier cause | Preserve the earlier attempt cause and do not relabel it interrupted; confirmed release still removes the workspace | Manual-entry cleanup-hold, SIGINT during SIGTERM grace |
| Sealed attempt; SIGINT/SIGTERM during bounded version lookup | Finish that lookup's own group cleanup; the first signal is the attempt cause because none was stored yet; a later signal does not replace it; then the probe group uses the shared owner | Manual-entry complete provider with version-stall, SIGINT then SIGTERM |
| Parent SIGKILL or crash | No handler runs; in-process cleanup of the detached group or workspace is not claimed | Supervisor boundary; no in-process pass |

The sealing boundary covers complete frames and partial bytes already delivered
in the same decoder chunk as close. A later chunk is outside the recording after
sealing; the stream is drained without retention. The owner records the first
failure before cleanup and never clears it. POSIX group confirmation excludes
zombies and escaped groups, as documented below.

## Refreshing evidence locally

Only maintainers refreshing a provider contract need to run live sessions:

```sh
node scripts/subagent-contracts/capture.mjs /tmp/subagent-selected.json
```

Interruption regressions launch this same process with `--controlled` and the
scripted provider. That entry does not call the live model or require credentials.
It is not a second cleanup implementation.

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

Raw frames exist only in memory. After the matching close response and the rest
of its decoder chunk are checked, the owner seals opening/prompt/close facts and
raw frames into immutable serialized evidence. Only this snapshot can be selected;
then raw data is discarded. Later stdout is drained without recording, including
while bounded version metadata is fetched. The selector keeps only the
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

Provider metadata is projected separately from tool evidence. Agent identity must
match the trusted installed adapter; capabilities retain known presence flags,
and mode values/kinds use the pinned closed vocabulary. Names, descriptions,
titles, unknown fields and nested metadata are not copied from the provider.
Source bytes are pinned to LF by `.gitattributes` so provenance is stable under
Git's platform line-ending conversion. The retained frames and `capturedAt` are
the October 6 live run. `probeSha256` is the checkout lock for `capture.mjs`;
the interruption supervisor realigned that lock without repeating the provider
session.
