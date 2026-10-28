# 1201 — Optional background tasks

Status: accepted for implementation. Issue: [#1201](https://github.com/nessalabs/nessa-agent/issues/1201).
Shepherd primitive: [shepherd#21](https://github.com/nessalabs/shepherd/issues/21).

## Decision

Offer background task tools alongside normal shell execution. Starting a background command returns a task ID promptly. The gateway owns the task independently of the agent harness, so the calling turn may continue or finish and the provider may park. Tool descriptions explain the two lifetimes neutrally without recommending background execution by default.

Shepherd owns process scopes, bounded output, cancellation and physical cleanup. Nessa owns the durable task identity, original conversation/execution/caller, admission audit and completion delivery. A task does not automatically restart its command. Gateway shutdown asks running tasks to stop and records their outcomes; abrupt host loss is recovered as interrupted/uncertain rather than inventing an exit or rerunning effects. Surviving gateway shutdown is not the same requirement as surviving agent parking.

The initial execution adapter runs commands in the canonical local workspace of a local conversation. It refuses a remote/boat placement rather than running its commands on the gateway's machine. Existing remote command leases follow agent leases and therefore cannot be detached unchanged; this feature must not silently weaken those leases.

## Presentation and continuation

The MCP start response carries the task ID. Before starting the process, the gateway resolves the unique pending or running native call in the originating execution by MCP server/tool and validated native input. That binding is saved before effects. Forwarded MCP result arguments arrive too late for this admission boundary. Ambiguous matching calls are refused; a retained retry returns the original binding even in a later turn. The existing tool waterfall continues to show that original call as running after the MCP start operation returns. Its details and terminal status are replaced with the retained background outcome when the task settles. No second tool row or separate task list is added.

Completion is stored before delivery. A fixed delivery execution ID makes retry converge on the same queued turn. An application-generated internal sender distinguishes completion input from a person's message in persisted SDK history. Provider delivery remains normal prompt continuation of the same session; this is not a late reply to an interrupted native tool call. The user-message projection omits internal input, while assistant output is rendered normally. Tool output remains visible on the original waterfall entry.

The internal completion is data from a tool, framed as such, and never system instructions. A busy agent queues it. Permission to continue is re-evaluated against the retained caller and current conversation access. A deleted conversation receives no continuation.

## Ownership and API

A gateway-built MCP server exposes `start`, `status` and `cancel`. The relay supplies session identity; arguments cannot name another conversation. Start takes a shell command, task ID and a timeout from one second to one hour (default two minutes). The command byte bound is derived from the SDK native-input bound with room for JSON escaping and other fields. A stable task ID enables intentional retry; distinct requests remain distinct tasks even when their command text matches. At most 32 tasks globally and four per conversation retain capacity while running, awaiting delivery or lacking confirmed cleanup. Each task retains a 16 KiB byte tail before UTF-8 decoding. Finished records live until conversation deletion; transition audit survives deletion. A restart marks unfinished tasks interrupted without claiming cleanup or automatically replaying their commands. Unknown cleanup blocks confirmed deletion and retains capacity; automatic process adoption and reconciliation are outside this implementation.

The ordinary shell tool retains its current behavior. No native shell feature flag is changed by this work.

## Orderings and evidence

| Row | Event/order | Required behavior |
| --- | --- | --- |
| T1 | Start while command blocks | Return task ID; original call projects running; subsequent agent work can proceed |
| T2 | Command exits before start reply | Retain outcome; original call resolves from that outcome once correlated |
| T3 | Caller disappears before admission | No process effect |
| T4 | Caller disappears after durable admission | Task retains ownership; retry returns same task |
| T5 | Agent ends/parks while task runs | Task continues; completion resumes same session |
| T6 | Agent is already busy at completion | Queue one internal continuation without interrupting current work |
| T7 | Completion is saved, acknowledgement lost | Replay same delivery identity; no duplicate command or continuation |
| T8 | Cancel races exit | Preserve first local cause and actual physical exit independently |
| T9 | Spawn/timeout/cleanup fails | Retain typed outcome; waterfall does not claim success or verified cleanup |
| T10 | Output overflows | Keep bounded tail and report omitted bytes; the model continuation explicitly says if its smaller input limit cuts the retained result |
| T11 | Another session supplies a task ID | Refuse through current ownership policy; no output or cancellation effect |
| T12 | Conversation deleted | Stop its tasks; no new agent continuation; cleanup uncertainty blocks false completion |
| T13 | Gateway orderly shutdown | Stop tasks before releasing process supervisor; pending completion remains durable |
| T14 | Gateway abruptly disappears | Recover nonterminal record as interrupted/uncertain; never replay its command |
| T15 | Store/audit fails at admission | Do not spawn |
| T16 | Terminal save fails | Keep ownership/result and retry while the gateway is active; do not notify from unsaved state |
| T17 | Provider echoes native MCP completion | Background projection remains running until authoritative task outcome |
| T18 | Internal continuation restored/replayed | Actual output reaches provider; user bubble remains absent |
| T19 | Status/read/cancel against retained terminal task | Return original evidence; no new process or continuation |
| T20 | Unsupported remote placement | Refuse explicitly; do not run in a different workspace |
| T21 | Native start input arrives before its MCP result | Bind using native input; do not wait for forwarded result arguments |
| T22 | Delivery succeeds but its status save fails | Retry the status save without another continuation |
| T23 | Two calls match one proposed identity | Refuse ambiguous first admission; effects remain absent |
| T24 | Stop audit fails for one task during bulk shutdown | Stop every live owner, shut down supervisor, return targeted failures; retain first stop intent for retry |
| T25 | Restored or adapter outcome contradicts its cause | Refuse settlement/restoration before releasing capacity |
| T26 | Conversation deletion removes current task row | Admission audit retains command, deadline, workspace and complete original binding |
| T27 | Desktop provides a configured agent PATH | Background execution uses the existing server environment owner |
| T28 | Export or retirement queues while background start holds admission | One admission owner spans authorization and task admission; no nested read behind a queued writer |
| T29 | Task identity contract changes | Product schema publishes the bounds and pattern consumed by Rust, MCP and the client |
| T30 | Stored JSON disagrees with indexed identity or retained state | Refuse restoration and reads before target selection, resumption or capacity decisions |

Tests belong beside their owning domain, application, infrastructure and public protocol boundaries. Conversation stress covers start, agent turn completion, task completion, one internal continuation and settlement of the original waterfall entry. The real harness spike is supporting evidence only; it does not replace these production tests.


## Evidence map

`tests/conversation/background_tasks.rs` covers T1, T3–T7, T12–T14,
T17–T18 and T21–T23 through the production service with controlled provider and
process ports. `background_store.rs` covers atomic admission/audit and immutable
binding; `background_lifecycle.rs` covers stop/outcome/delivery agreement.
Shepherd's public `background_tasks` suite covers T2, T8–T10 and physical scope
shutdown. The stress `background` step exercises T1, T5–T7, T17–T18 and T21
through the real gateway, local process adapter and both browser engines.

The task result carries an OS PID only after an observed outcome; that PID is
diagnostic and is never used as cancellation authority. After host loss, the
record cannot identify or adopt a surviving process attachment. This limitation
is reported as interrupted/unknown cleanup, not a successful command or release.

## Review ownership correction

The second code review found that task start and tool authorization each owned an
admission read, and that persisted indexed facts were trusted separately from the
record. The revised boundary passes one held admission into shared authorization.
Storage uses one checked row decoder, validates all rows when opening before
indexed recovery, and rejects disagreement without rewriting evidence. Task ID
syntax is published from the product schema for every consumer.
