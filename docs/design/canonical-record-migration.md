# Canonical conversation records and legacy import

**Status:** proposed cutover contract for [#274](https://github.com/nessalabs/nessa-agent/issues/274), under [#258](https://github.com/nessalabs/nessa-agent/issues/258). The baseline codec reuses the SDK's typed snapshot field mappings. An adapter writes an opening record, bounded pieces and a seal through the `event-stream` runtime, and can read the sealed baseline after restart. The current SDK still writes leased JSONL session journals; no production record runtime or authority switch is composed yet.

## What becomes authoritative

One SDK coordinator owns decisions about a conversation. Once the transition is complete, it commits Nessa semantic records to one `event-stream` runtime before applying the committed fact or starting a new effect. The gateway may authorize and read those records; a phone may cache and fold them. Neither a gateway reader nor a phone replays a record by executing a prompt, tool or agent.

The library owns the generic `StreamKey`, incarnation, cursor, event ID, append identity, bounded reads, replay/live subscriptions and SQLite store lifetime. Nessa owns what the payload means, verified caller attribution, command receipt lookup, provider correlation, deletion and recovery decisions. The current pinned library revision is `b39790230c21ae797b05f54e29a7b8dc51d89766`, merged in [event-stream PR #2](https://github.com/nessalabs/event-stream/pull/2). Production SDK code enables its `codec` feature for ACP JSON-RPC framing; the SDK test graph also enables `sqlite` to prove restart recovery. Its durability and lifecycle guarantees must pass Nessa integration tests before composition relies on them. The library's optional replication feature is a separate protocol; the device path uses `nessa-sync`'s current record contract.

```mermaid
flowchart LR
    Provider[Provider adapter] --> SDK[SDK conversation coordinator]
    SDK -->|commit semantic fact| Stream[event-stream runtime]
    Stream <--> SQLite[Owned SQLite record store]
    Stream -->|committed bounded read| Gateway[Gateway authorization adapter]
    Gateway -->|authorized range| Device[Device cache and fold]
    SDK -->|after commit| Effect[Provider or tool effect]
```

## Current evidence and the migration limit

`SessionSnapshot` retains the session/provider identity, context, invocations in admission order, their request and verified `ActionContext`, acknowledgement, observations, scheduling, cancellation, provider report, local outcome and result, plus global queue history. `LocalFileStorage` appends changed JSONL snapshot lines under a per-session writer lease. Replay of complete lines reconstructs and validates the latest snapshot; an incomplete final line is truncated, and a malformed complete line is refused.

The snapshot does **not** retain a total historical order between observations and scheduling changes in different invocations. Import can preserve the current facts, each invocation's own order and the saved queue order. It cannot invent a past global event timeline. The first migrated record is therefore a clearly identified **baseline**, followed by actual new semantic changes. A device can show the recovered transcript and its current state, but must not present that baseline as a minute-by-minute historical event feed. Provider context is recovery metadata for the SDK, never a remote execution capability.

The gateway's conversation catalogue and deletion state remain owned by its conversation store. Audit is independently owned today. Their facts must be captured or linked through explicit identity and cut evidence; copying an SDK snapshot cannot prove that a conversation still exists or that a file is authorized. Before exposing a loaded baseline, composition compares its saved session ID with the requested conversation and checks the gateway ownership/deletion cut. The import cannot silently resurrect a deleted conversation.

## Record and import contract

The Nessa payload schema is versioned independently of `event-stream`'s generic envelope. It needs these families:

| Family | Meaning and owner | Required identity |
| --- | --- | --- |
| Imported baseline | Coherent validated legacy session at a frozen cut; SDK import owns it | Session ID, which names the journal under its lease, and the validated cut digest |
| Command accepted | Exact `ExecutionRequest`, submission mode, verified `ActionContext` (including caller `requestId`) and initial acknowledgement; SDK admission owns it | Conversation, caller/operation/target-qualified `requestId`, and stable `execution_id` submission key |
| Provider observation | Normalized `ExecutionEvent` after the SDK accepts its source identity | Conversation, owning `execution_id` and zero-based observation ordinal |
| Scheduling/stop decision | Queue order, local stage, cause, actor and target, separate from provider acknowledgement | Conversation, owning `execution_id` and per-invocation transition ordinal; queue decisions use stream order |
| Settlement/recovery | Provider report, local result, cleanup and uncertainty as separate facts | Conversation and owning `execution_id`; a superseding fact names the fact it corrects |

For a newly created conversation, the SDK first commits its session identity, provider identity and recovery context as the stream's opening fact. An imported conversation gets those values from its sealed baseline. `ActionContext` retains the host-verified caller, surface and `requestId`; `ExecutionRequest.execution_id` names the SDK submission. An identical retry must match both identities, the exact input and submission mode; a mismatch is a conflict. Do not invent a request or attempt ID for a legacy fact that did not have one. The gateway conversation catalogue owns whether that conversation exists and who can see it. Gateway deletion must leave a durable tombstone or equivalent deletion fence in that owner before a migrated or cached SDK record can be exposed again. The exact gateway deletion transaction belongs to [#275](https://github.com/nessalabs/nessa-agent/issues/275); a baseline cannot make a deleted conversation live.

Before a new admission, the writer looks up the caller-qualified `requestId` for that operation and target, then the proposed `execution_id`. A matching saved acceptance returns its original receipt. Reusing either identity with different input, mode or attribution is a typed conflict. This lookup is a projection of the same committed stream and baseline; it is not a second receipt database. The currently leased SDK storage can recover by `execution_id` and exact `ActionContext`; #275 must preserve that behavior and add the ADR 0008 command lookup before it allocates or dispatches work.

The baseline may need several bounded payload records because a validated legacy snapshot can exceed one event limit. Its opening record names the source session and digest of the validated semantic cut; numbered pieces carry exact bytes; a final seal states their count, total bytes and digest. The fold exposes a baseline only after checking that seal and the opening identity. Deterministic event IDs and exact retry bytes let an interrupted import resume without duplicating pieces. The payload uses Nessa-owned typed snapshot field mappings and bounds, not a second copy of the JSONL journal contract. The importer checks that decoding the pieces reconstructs a `SessionSnapshot` passing the same application validation as the source.

The current version 1 record payloads are byte encoded:

| Schema | Payload fields in order | Meaning |
| --- | --- | --- |
| `nessa.baseline-open` | version `u8`, session ID length `u16`, session ID UTF-8, cut digest `[u8; 32]` | Names the legacy session and exact validated semantic cut. |
| `nessa.baseline-piece` | version `u8`, section `u64`, part `u32`, byte length `u32`, exact bytes | Carries one bounded section fragment. |
| `nessa.baseline-seal` | version `u8`, piece count `u64`, content bytes `u64`, digest `[u8; 32]` | Commits the opening record's cut after all pieces match. |

Integers use big-endian bytes. The digest is SHA-256 over each piece's section, part, length and content in order. Event IDs derive from that digest and the record index. The stream importer reads an existing prefix before appending, refuses a different prefix, and reads back the full prefix after the seal. An identical late retry can recover that seal cursor even after newer facts follow it. A missing seal leaves the legacy journal authoritative. A sealed read returns both the reconstructed snapshot and the seal cursor, so the later fold starts after the baseline. The SDK's SQLite restart test reads the sealed baseline back through a newly opened `event-stream` runtime; the production gateway cut and writer switch remain [#275](https://github.com/nessalabs/nessa-agent/issues/275).

Every later record is immutable. A correction or superseding outcome is a new fact with explicit cause and target; the old fact remains. Command receipt lookup uses the committed acceptance record and never dispatches work. A record from another stream incarnation, an unknown required schema or an impossible combination of actor, target and result stops the fold with a typed error before checkpoint advance. Optional display content may be reported unsupported without changing lifecycle meaning.

### Incremental record ownership and commit order

The live writer in [#275](https://github.com/nessalabs/nessa-agent/issues/275) must derive incremental records at the existing `SessionManager` decisions, not by diffing an arbitrary final snapshot. One conversation stream serializes commits; the stream cursor gives the order of *committed* facts. A record carries its stable event ID, schema version, owning invocation identity where applicable, and the exact validated value below. An invocation's request supplies its target, so later records refer to that invocation rather than copy a second request or actor. The baseline seal cursor is the starting cursor for these records.

| Commit boundary in the current SDK | Canonical fact and fields | Required order / interpretation |
| --- | --- | --- |
| `begin_record` saves admission | `input-accepted`: request and execution identity, submission mode, verified `ActionContext`, initial `SubmissionAcknowledgement`; include the first `Submitted → Queued` edge when queued or steering | Commit before scheduler membership or provider work. An unacknowledged receipt is still an accepted input; it must not be redispatched on replay. |
| Queue membership or selection is saved | `queue-decision`: `QueueMutation`, optional verified actor, affected invocation and scheduling prefix | Preserve the global queue order. Selection does not claim provider dispatch. |
| `record_scheduling` saves a transition | `scheduling-transition`: invocation identity and exact `InvocationSchedulingEvent` | Validate `before`, stage, cause, target and actor against that invocation's prior edge. `Dispatched` precedes polling ordinary provider work; `Injected` follows provider acknowledgement of native steering. |
| `event` accepts and saves a provider observation | `provider-observation`: invocation identity, ordinal and exact normalized `ExecutionEvent` | Commit only observations owned by a dispatched invocation. Message fragments can remain live-only until the next durable save boundary; do not claim a lost fragment was committed. |
| Submission acknowledgement changes | `receipt-updated`: invocation identity and exact `SubmissionAcknowledgement` | This reports audit/storage acknowledgement, independently of provider acceptance or the final execution result. A failed acknowledgement retains its failure detail. |
| A stop is saved | `stop-decision`: invocation identity, whether it is undispatched cancellation or the first local stop of dispatched work, cause and verified actor if explicit | A local stop does not prove provider cleanup. Retain the first stop even when a later provider report or local failure arrives. |
| `record_provider_report` saves a reply | `provider-report`: invocation identity and exact `ExecutionReport` | This is the provider's settlement evidence. Local cancellation requires an earlier owned stop; absent report remains unknown, not failed. |
| `finish` saves local settlement | `local-settlement`: invocation identity, saved `result` and any earlier `local_outcome` it preserves | Do not overwrite provider evidence with hook, audit or storage failure. A later correction is a targeted new fact, not in-place replacement. |
| Provider context changes under SDK ownership | `provider-context`: validated `ProviderContext` and owning session identity | Recovery metadata stays on the trusted host and must not be exposed as a device execution credential. |

The current snapshot API can save several changed fields together. The live writer must either commit one atomic Nessa record containing that whole transition or commit a documented sequence whose intermediate prefixes are valid. In particular, admission and its first queued edge are one fact, while queue membership is a later fact. The current native-steering path can receive provider acceptance before its `Injected` edge or receipt acknowledgement is saved. A crash in that gap leaves a pending accepted input with an **uncertain delivery outcome**; recovery must inspect the provider's correlation or retain uncertainty, never retry injection merely because the edge is absent. Each committed prefix must fold to a valid state and must never start a provider or tool effect during replay.

The `event-stream` SQLite and memory stores default to a 1 MiB accounted record limit, while `ExecutionRequest` permits 4 MiB of UTF-8 message text. The live format therefore needs a bounded large-fact path before #275 changes admission. Small facts should take one append. For a larger fact, append an opening identity and digest, numbered bounded pieces and a seal; expose and apply the fact only after the seal validates. A partial large fact leaves the previous committed conversation state intact. The fact identity must derive from the SDK's stable operation/field identity, not its content digest: retrying the same identity with changed bytes is a conflict, not a second accepted input. The version 1 format below settles the payload, bound and retry IDs; #275 must implement and test them against the configured store limit and the 4 MiB SDK input boundary.

### Version 1 live fact format

The logical body is UTF-8 JSON encoded from typed Nessa fields. Its nested `Provider`, `Metadata`, `Event`, `QueueEvent`, `SchedulingEvent`, `Acknowledgement`, `Settlement`, cancellation, outcome and error values use the **same mappings** as `snapshot/`; there is no second serializer for those values. The writer serializes the typed body once and retains those exact bytes through an uncertain append. The reader bounds and decodes the complete body, validates its identity against the frame and prior state, then advances its checkpoint. A decoder must reject unknown required fields, versions and fact kinds rather than guessing what they mean.

| Kind | Identity within the conversation stream | Typed JSON body and constraint |
| --- | --- | --- |
| 1 `session-open` | No execution ID, ordinal 0 | Session ID, `Provider`, provider context; invocation count is zero. A new stream starts here instead of with an imported baseline. |
| 2 `input-accepted` | `execution_id`, ordinal 0 | Admission `Metadata` and its optional first `SchedulingEvent`; no observations, provider report, stop or result yet. The metadata's execution ID must match the frame, and its `Actor.request_id` is retained for caller-qualified receipt lookup. |
| 3 `queue-decision` | No execution ID, ordinal = prior queue-history length | One `QueueEvent`; its mutation identifies the affected execution, and its saved scheduling prefix is checked against that invocation. |
| 4 `scheduling-transition` | `execution_id`, ordinal = prior scheduling length | One `SchedulingEvent`, validated against the previous stage and the admission kind/target. |
| 5 `provider-observation` | `execution_id`, ordinal = prior observation length | One `Event`, whose execution ID must match the frame. |
| 6 `receipt-updated` | `execution_id`, next receipt revision | Previous and next `Acknowledgement`; the previous value must equal the folded value. |
| 7 `stop-decision` | `execution_id`, ordinal 0 for undispatched stop or 1 for first dispatched local stop | Stop scope and the existing cancellation value; a repeated scope conflicts. |
| 8 `provider-report` | `execution_id`, ordinal 0 | One `Settlement`; it cannot replace an earlier provider report. |
| 9 `local-settlement` | `execution_id`, next local-result revision | Previous and next saved result plus retained `local_outcome`; a later local failure keeps earlier successful outcome and names the result it supersedes. |
| 10 `provider-context` | No execution ID, next context revision | Previous and next provider context; host recovery metadata stays out of device views. |

The frame key is `(kind u8, execution ID length u16, execution ID UTF-8, ordinal u64)` with integers in big-endian order; absent execution ID has length zero. For a given stream, the physical event ID is `nessa-fact-` plus lowercase hex SHA-256 of those key bytes, followed by `-start`, `-part-<zero-based index>` or `-seal`. Thus a retried fact uses the same start ID even if its bytes or inline/chunked choice changed; `event-stream` reports that as an identity conflict. Revisions advance only after replay has established the prior committed fact, including after an uncertain reply.

Every physical record uses schema version 1 and one of `nessa.fact-start`, `nessa.fact-piece`, or `nessa.fact-seal`. A start payload has, in order, frame version `u8`, frame key, body length `u64`, piece count `u32`, SHA-256 of the complete JSON body, mode `u8`, then any inline body bytes. Mode 0 has zero pieces and carries the complete body in that record. Mode 1 carries no body in the start record; consecutive pieces each carry version `u8`, index `u32`, byte length `u32` and at most 64 KiB of body bytes. The seal carries version `u8`, piece count `u32`, body length `u64` and the same digest. The writer uses mode 0 only when the complete start record payload fits 64 KiB; otherwise it uses mode 1. Body length is capped at 160 MiB, matching the existing baseline section bound. A chunked fact becomes visible only at its validated seal cursor; an inline fact becomes visible at its start cursor. The writer allows only one unfinished fact per conversation stream, so another fact cannot appear between its start and seal.

| Saved prefix or retry | Fold and writer action |
| --- | --- |
| Inline start with valid body | Apply once at its start cursor. An identical retry returns that cursor. |
| Chunked start and incomplete pieces | Apply nothing; resume missing pieces with the same IDs and bytes. Hold later facts and any effect that depends on this one. |
| Complete pieces without seal | Apply nothing; append or locate the matching seal. |
| Valid seal | Decode and apply the one fact at the seal cursor. Later records start after it. |
| Same fact key with changed bytes or mode | Refuse the conflicting command; do not allocate an alternate ID or dispatch. An SDK submission retry must retain its original `execution_id` and verified `ActionContext`, including `requestId`. |
| Unknown fact kind/version, bad sequence or bad digest | Stop the fold with a typed error before checkpoint advance. No replay path executes a provider effect. |

These are the wire and ordering decisions for #274. #275 implements this frame and the SDK writer, checks the store's reported record limit before admission, and proves that a 4 MiB command remains one logical acceptance. A store configured below the maximum accounted physical record size must fail composition with a typed error. #276 may send bounded pieces to a device; #277 exposes a fact to the view only after the matching seal.

```mermaid
sequenceDiagram
    participant C as Client
    participant G as Gateway
    participant S as SDK coordinator
    participant R as event-stream runtime
    participant P as Provider
    C->>G: Submit requestId, executionId and input
    G->>S: Verified caller and canonical input
    S->>R: Append fact start with stable ID and digest
    loop Only when the fact exceeds the inline limit
        S->>R: Append next bounded piece
    end
    opt Chunked fact
        S->>R: Append matching seal
    end
    R-->>S: Confirm complete fact cursor
    S->>S: Apply accepted input and retain receipt
    S->>P: Begin authorized provider work
    S-->>G: Return original receipt
    G-->>C: Acceptance response may be lost
    C->>G: Retry same requestId and executionId
    G->>S: Look up committed receipt and compare input
    S-->>G: Original receipt without dispatch
    G-->>C: Original acceptance response
```

## Cutover states and orderings

The legacy lease and new runtime cannot jointly accept writes. On startup, composition chooses the source of authority from a durable sealed import marker in the target stream. If it is absent, acquire the legacy writer lease and stop admission for that session while importing. A sealed target stream is authoritative even if the old journal still exists; retiring the old bytes is a later cleanup and must not affect the decision.

| Saved state / event | Allowed transition | Effect and recovery assertion |
| --- | --- | --- |
| No target stream; valid legacy journal | Freeze admission and acquire its lease; begin import | Source remains the only authority until the seal commits. |
| No target stream; no legacy journal | Create an empty new stream through normal admission | Do not fabricate an imported conversation. |
| Legacy load is malformed or identity mismatched | Refuse import | Do not interpret it as empty; leave source and admission blocked for repair. |
| Piece append is rejected | Stop import | No seal; readers ignore the partial target. |
| Piece append outcome is uncertain | Look up the same event ID and exact bytes, then retry or stop | Do not allocate a different ID or switch authority on a timeout. |
| Crash after some pieces, before seal | Reacquire the same legacy cut and resume deterministic pieces | If the source cut changed, refuse and investigate; no mixed baseline. |
| Seal append confirmed; old journal still present | Select target stream as authority, then release legacy lease | Never write new facts to the old journal again. |
| Crash after seal but before cleanup | Select target stream on restart | Cleanup cannot turn the old journal back into authority. |
| Legacy deletion races with import | Coordinate with the gateway deletion owner and refuse an obsolete cut | A copied session cannot override the deletion fence. |
| A new prompt arrives during import | Hold or explicitly refuse admission | No provider effect occurs before the target authority is ready. |

```mermaid
sequenceDiagram
    participant Host as Gateway composition
    participant Old as Legacy leased journal
    participant New as event-stream runtime
    participant SDK as SDK coordinator
    Host->>Old: Acquire writer lease and validate saved session
    Host->>Host: Freeze admission and confirm ownership/deletion cut
    Host->>New: Append opening identity and source-cut digest
    loop Bounded deterministic pieces
        Host->>New: Append piece with stable event ID
        New-->>Host: Inserted or identical retry
    end
    Host->>New: Append sealed import marker
    New-->>Host: Durable receipt
    Host->>SDK: Select target stream as sole authority
    Host->>Old: Release legacy lease
    SDK->>New: Commit next semantic fact
    SDK->>SDK: Apply committed fact
    SDK->>SDK: Start authorized effect when required
```

## Verification required before cutover

1. A fixed version 1 JSONL fixture with accepted input, transcript, provider settlement and acknowledgement imports through SQLite and reconstructs after restart. A separate generated multi-invocation case retains unfinished input, output, queue history, cancellation and distinct provider/local outcomes through the same restart path.
2. A partial final JSONL line recovers only the committed prefix; a malformed complete line, wrong session ID, missing provider identity and contradictory acknowledgement/result each produce typed refusal.
3. Interrupted piece append, lost commit reply, altered retry bytes, source-cut change and a crash after seal exercise every table row without two active writers.
4. Compare the gateway's old view with the new fold over the imported baseline. Check that a copied fact invokes no provider, tool or host effect.
5. Verify source deletion/ownership at the import cut and again before exposure. The product deletion and backup contracts remain separate issues.

[#275](https://github.com/nessalabs/nessa-agent/issues/275) owns the production write-path switch; [#276](https://github.com/nessalabs/nessa-agent/issues/276) owns bounded sync reads; [#277](https://github.com/nessalabs/nessa-agent/issues/277) owns the shared fold. This issue owns the baseline schema, importer and migration evidence that those slices depend on.
