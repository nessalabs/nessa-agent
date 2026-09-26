# ACP transport and execution delivery

The infrastructure split is:

| Module | Owns |
| --- | --- |
| `claude_acp` | Anthropic model ceilings, Claude launch environment, pinned harness validation, settings/mode enforcement, Claude tool-name metadata and raw input schemas. |
| `acp` | Shared configuration, application port handles, ACP startup/prompt/cancel/permission exchange, standard message/tool translation, and an injected provider profile. |
| `json_rpc` | Envelope validation, numeric/string request correlation, bounded newline reads and writes. JSON/serde remain here and in protocol adapters. |
| `process` | Restricted subprocess group ownership, pipe draining, termination, reaping, and cleanup evidence. The caller supplies the launch command. |

The shared runtime is exercised with `test_acp_handler.py` using no Claude metadata,
a different tool input schema, and its own initialization/configuration behavior.
It is reuse evidence, not support for another production provider. Live Claude
compatibility still depends on the pinned harness and supported restricted profile.

### Event-stream reuse

The SDK pins `nessalabs/event-stream` at
[`66ba7525260040d6265276692088dca6dae0737e`](https://github.com/nessalabs/event-stream/tree/66ba7525260040d6265276692088dca6dae0737e)
with only its `codec` feature. JSON-RPC reads use the library's `NewlineFramer`
and `IncrementalDecoder`, preserving partial frames across cancelled reads and
bounding frame size, decode work, and queued input. Tests cover fragmented UTF-8,
CRLF, whitespace, multiple frames, exact size limits, truncation, and write deadlines.
After framing, JSON parsing permits at most 65,536 items per frame: every value
(including each array/object itself) and every object key counts once. This separate
allocation/work budget prevents tiny collections from expanding into millions of
heap values within an otherwise valid byte limit. It applies to ignored metadata
as well as retained fields; strings remain bounded by the frame byte limit, and
Serde's recursion limit remains enabled. Exceeding any parser limit fails the frame
before dispatch with a bounded protocol error.
JSON-RPC successful `result: null` is preserved distinctly from an absent result.
Repeated JSON object keys are rejected at every nesting depth, including equal
values and escaped spellings of the same key, before policy validation. Permission
requests require a non-null RPC identifier during startup and execution; malformed
requests fail the binding and trigger cleanup even without an execution timeout.

The library's ingestion service maps decoded items to appendable events; it is
not used for bidirectional ACP control traffic. ACP responses and permission
requests are handled live, not replayed as effects. The current `ExecutionEventStream`
reader remains a bounded ephemeral observation port. Integrating the library's
storage/replay runtime for committed conversation records remains
[ADR 0009](../../../../docs/adr/todo/0009-reusable-event-stream-crate.md).
The binding constructs no SQLite store or conversation replay runtime. Agent's
injected SessionManager separately saves local snapshots of its observations.

## Streaming, permission, and Close contracts

Each prompt carries a host-owned `execution_id`. Every observation and terminal
`Finished` update carries that ID, so queued updates cannot be attributed to the
next prompt. Permission answers must match execution, request, and offered option IDs.
At this raw adapter boundary, execution IDs correlate provider attempts; the
transport does not deduplicate requests. Public Agent queue/steering admission
owns [idempotent submission recovery](scheduling.md#idempotent-submission-retries),
so a retry never becomes a second wire prompt. Only one prompt may be active per
live session; raw overlapping execution returns `Busy`. Commands and
events use bounded channels. Frames and persistent identifiers have size limits.
The application controller caps retained tool payload at 32 MiB and pending review
payload at a separate 32 MiB. Tool accounting includes the entity identities and
the aggregate lookup key; review accounting includes the request’s execution,
permission, and tool identities, both pending lookup keys, offered choices, and
scoped identities. Sparse updates retain existing
charges; replacement, review resolution, and execution settlement release the
corresponding allocations.
It admits at most 4,096 tools, 128 pending reviews, and 4,096 permission identities
per execution. Seen permission identities are separate history bounded by the
identity-size and admission-count limits; fixed struct/map storage is bounded by
collection counts. Resolved identities count until finish so once-only protection
cannot grow without bound. These are retention limits, not a total process-memory
ceiling: queued events, parsing buffers, and consumer-owned results are separate.
`execution_timeout: None` is the default policy: a prompt has no elapsed-time cutoff.
A host can explicitly opt into a total runtime limit with `Some(duration)`. That
limit starts when the worker selects the request for policy validation and carries
through prompt writing and execution without restarting.
Elapsed runtime or quiet output is not a stuck-agent diagnosis. Startup, protocol
writes, and cleanup keep their separate deadlines, and Close remains available
throughout an unlimited prompt. Health assessment is separate future work.

Decoded output has a separate 32 MiB byte budget shared by all process generations
of one attachment, alongside the configured event-count limit. Charges include
owned payload capacity and event storage; dequeue, receiver drop, and failed enqueue
release them. Undrained events from a closed generation still consume the shared
budget after restoration. Transient JSON decoding, domain state, channel/allocator
overhead, and consumer/audit-owned copies are outside this queue budget. Agent
separately bounds retained observations per invocation to 128 MiB and 262,144
events, so actively draining the queue cannot bypass retained-output admission.
The queue and retained-history checks share the same event payload accounting;
the queue additionally charges its permit storage. See [Agent retention](agent.md#ui-and-tests)
for cutoff, audit, and same-Agent recovery behavior.

Before prompt or native-steering dispatch, the worker handles ready provider
messages through its normal policy and observation handlers. Processing yields in
bounded batches; a finite valid burst does not fail merely because it spans a
batch. An uninterrupted ready stream delays dispatch until no messages are ready,
while close, caller loss, consumer loss, and deadlines remain responsive. Decoder
fairness yields and task-budget exhaustion are not treated as absent provider input.
A partial frame genuinely waiting for more OS input does not block dispatch; this
boundary covers complete ready frames and decoder work on already-ready input.
The native-steering operation deadline likewise includes this policy-validation
period and is not restarted for writing or acknowledgement.

Outgoing prompt and native-steering frames are encoded and checked against the
configured byte limit before reserving a wire ID, beginning an execution, or
installing a pending reply. Oversized local input returns `InvalidInput` without
retiring the worker or interrupting an active turn; repeated rejection does not
accumulate retired event readers. Prompt and native-steering writes also observe
the active execution deadline. Live permission answers, withdrawals, unsupported
requests, and early permission rejections use the earliest active execution or
steering deadline. Ready messages handled before dispatch retain the selected
command's deadline for their response writes. Startup and restoration writes, including replies to provider-originated requests,
share the absolute startup deadline. A blocked write
expires at the earlier operation deadline or the one-second transport write bound.
The operation deadline starts cleanup; it does not cap subsequent cleanup or audit
delivery time. With no execution timeout, the one-second write bound still applies.
Actual write failures still require cleanup because partial delivery is uncertain.

Every one of these deadlines, and the audit record's bound, is a moment on
`AcpConfig::clock` (`infrastructure::clock`): composition supplies
`RuntimeClock`, and tests a clock that moves only when the test moves it, so a
deadline passes exactly where a test says and never because the machine was
slow. Waiting for the process to exit and reaping it stay on real time, since
they wait on the operating system rather than the agent.

Output queue count or byte overflow is a binding failure; events are not silently dropped while
execution continues. The worker tears down the scope, then the reader reports the
failure after previously queued events. Advisory usage/plan/metadata extensions
are bounded and ignored; they cannot change capabilities or imply success.

Tool events are sparse patches: `None` means omitted, while `Some([])` replaces a
collection with an empty collection. Text and file diffs are normalized into
Nessa-owned content. Permission events contain an application `ToolReviewInput` with the observed tool name
and complete arguments encoded as JSON text. The adapter validates the supported
schema without discarding fields. Domain permissions own correlation and decision
state, not a closed enum of provider tool schemas. A host policy must interpret the
actual schema; the example compares the entire Write input before allowing it. Those values are untrusted data for the host's
policy/review, never authority by themselves. Answers use local, session-scoped
IDs; already-answered or stale IDs cannot approve a later request.

A provider `$/cancel_request` withdraws only the matching pending permission.
The stream emits `PermissionCancelled(PermissionCancellation)` so the host can dismiss that review;
later answers are stale. Repeated, unknown, or already-resolved cancellation IDs
have no effect on other reviews or on the active execution.

Execution results distinguish completion, output limit, request limit, refusal, and
cancellation. An unknown stop reason is a protocol failure. Drain the ordered
`Finished` update as well as awaiting the prompt result: a result future can be
ready before its preceding text has been consumed. Admission rejections can
return without an event; fatal stream failure closes the scope and resolves any
pending prompt. A full queue that prevents terminal delivery fails both the
prompt and reader; no successful completion is reported without its event.
Transport failures produce port errors rather than fabricated domain completion
events. Stream exhaustion alone does not establish success. Before publishing
`Finished` and settling the result, the worker cancels pending reviews, records
their audit evidence, sends their wire cancellations, and releases execution
state. Failure in those steps prevents successful settlement.

Close records the live aggregate closure even if idle, closes pending permissions,
sends ACP cancellation, and waits
within the configured grace period. It then closes stdin, allows harness teardown,
sends termination to the owned group if necessary, escalates to a forced kill,
reaps the direct child, and verifies group disappearance. Stderr is drained
without retaining provider text. Protocol writes also have a one-second bound;
permission cancellation is bounded by the grace period. Startup failures,
provider/transport failures, deadlines, dropped consumers, and shutdown share
this cleanup path. Startup retains a known provider context before validating its
model/mode/configuration, so later startup failure still produces closure evidence.
Session updates received while startup RPCs are pending use the live session/profile
validator once a context is known. Updates before context admission fail closed;
startup ordering cannot hide model or permission-mode drift.
Explicit close during restoration preserves the caller and close cause; validation,
provider, and deadline failures retain their own causes. Attachment failure without
an active domain execution records `SessionFailed`; `ExecutionFailed` carries the
active execution identity. A worker may still own a result waiter after domain
settlement, so waiter ownership alone cannot justify execution-failure evidence.
The application supplies the actual shutdown cause through
`SessionCloseRequest`; sharing cleanup code does not collapse those audit reasons.
When operation failure, audit delivery, and process cleanup fail together, the
returned error preserves the primary typed failure and the teardown failures.

Close shuts down this live connection. A later `execute` restores the same provider
context through capability-gated `session/resume`, after successful cleanup.
Missing history, unsupported resume, audit failure, or uncertain cleanup prevents
execution instead of silently starting a fresh conversation. A fresh aggregate and
RPC state are created; the existing client and event reader remain usable.
The reader drains old connection events before new ones. `next() == None` means
that the current connection is drained; after starting the next execution, poll
that same reader again for the resumed connection. It does not await a hypothetical
future resume while the client remains closed.
There is no separate cancel-only execution method yet. Repeated
Close calls return the confirmed cleanup result. Uncertain process cleanup retains
the actual process scope; a later close retries it before permitting restoration.
Failed startup exposes that same ownership through `ProviderOpenError` and
`ProviderCleanup`, even without a provider session ID. If an opening waiter or the
last recovery handle is dropped after failed cleanup, a supervised retry task
retains the actual scope until termination and reaping are confirmed. It retries
physical cleanup without replaying lifecycle audits; the Tokio runtime must remain
alive until cleanup completes. Cleanup retry preserves
audit failures and does not redeliver missing evidence. `Cancelled` is withheld until cleanup
succeeds. A known completion can win the race with Close without being rewritten.
If cleanup cannot be established, the port reports `CleanupUncertain` and remains
closed. A provider error or idle protocol failure is distinct from successful
resource cleanup; the event reader and pending prompt preserve that failure.

Tool content decoding accepts supported text blocks and diffs only. An unknown
outer variant or unsupported nested block rejects the entire update; a partial
parse never becomes an empty replacement for earlier observations. Omitted
content retains its sparse-update meaning, while an explicit empty array clears
content deliberately. Provider tool-name state changes only after full decoding
succeeds.

ACP workspaces must be absolute UTF-8 paths because the session workspace crosses
JSON. Configuration rejects non-UTF-8 paths before launching a child. During
cleanup, in-flight session updates are drained without adding text, thoughts, tool
state, or configuration changes; terminal settlement and cancellation audit remain
on their dedicated paths.

Each permission cancellation audit record receives its own `shutdown_grace`
delivery deadline. A timed-out record reports `AuditFailure`; later records are
still attempted with a fresh deadline before UI publication. Bulk audit latency
can therefore grow with the bounded number of pending reviews.

Cooperative shutdown starts one `shutdown_grace` deadline. Initial cancellation,
fallback cancellation after failure, and waiting for process exit consume that same
interval; a retry does not start another grace period. Normal completion's pending
permission replies start this same grace interval and retain it if cancellation or
terminal publication fails; successful completion clears it for the next turn.
Once shutdown begins, old execution and steering timers cannot replace its cause. Expiry advances to forced
cleanup without replacing an explicit close's cause or an existing operation error.
An earlier one-second transport write timeout remains a failure. Forced termination
and reaping retain their `kill_timeout` bounds, while mandatory audit calls retain
separate per-record deadlines. The grace interval is therefore not a total bound
on close, audit delivery, and forced cleanup.
