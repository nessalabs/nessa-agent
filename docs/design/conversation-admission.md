# Proposed prepared Agent lifecycle

Issue [#92](https://github.com/nessalabs/nessa-agent/issues/92) proposes making
`conversation.create` and `conversation.send` independent of provider startup.
This note is a design candidate for review. It describes no implemented
guarantee.

The first candidate put a durable pre-provider queue in the gateway. That split
one scheduling decision between the gateway and the SDK and is rejected. The
smallest correct boundary is to prepare the existing SDK Agent before attaching
its provider. The same `Scheduler`, `SessionManager`, work generation, receipts,
and audit port then own input before, during, and after provider startup.

## One owner through both stages

```text
conversation metadata + creation audit
                 |
                 v
SessionManager lease/load --> prepared Agent --> provider attachment
                                  |                     |
                                  +-- one Scheduler ----+
                                  +-- one lifecycle ----+
                                  +-- one audit port ---+
```

Arrows are ownership transfers or calls. A prepared Agent is the Agent, not a
gateway queue or a second public scheduling handle. It accepts, removes,
reorders, closes, and recovers receipts through the existing SDK authorities.
Its queue runner cannot enter provider preparation or dispatch until attachment
is confirmed.

`conversation.create` still durably creates and audits conversation ownership.
It then acquires the SDK storage lease, loads and validates the snapshot, builds
the Agent, starts the existing runtime-readiness wait in a retained conversation
task, and returns. `conversation.send` calls that prepared Agent's normal queue
API and returns with the SDK's owned receipt plus its storage/audit
acknowledgement; it does not wait for readiness or `provider.open`.

## Minimal SDK surface

Replace the one-stage construction contract with these responsibilities:

```rust,ignore
let agent = Agent::prepare(provider, manager, execution_audit).await?;
let admission = agent.enqueue(request, actor).await?;

// Capture authority now; use it after the gateway's runtime-readiness gate.
let authorization = agent.authorize_attachment(attachment_request)?;
let attachment = agent.start_attachment(authorization)?;
let outcome = attachment.wait().await;
```

- `Agent::prepare` loads and validates storage, initializes the one scheduler
  and lifecycle coordinator, and retains the caller-owned execution audit. It
  does no provider I/O.
- `Agent::authorize_attachment` synchronously issues at most one opaque,
  single-use authorization for the current attachment and work generations. Its
  validated request carries a typed initial-open, reopen, or automatic-recovery
  cause plus the required caller identity and correlation for explicit causes.
  Issuing authority performs no provider I/O and does not make active work
  admissible.
- `Agent::start_attachment` is synchronous. Under the lifecycle state lock it
  consumes the authorization, verifies both generations and the open/cleanup
  state, changes the phase to `starting`, and installs exactly one Agent-owned
  attachment task before returning a join handle. Repeating it with the consumed
  token is a typed stale-authorization result; callers join through the returned
  handle. Dropping that handle does not drop `provider.open`; the Agent-owned
  task first delivers the attributed attachment-attempt audit, then retains
  startup, cleanup, event-reader installation, and the save that makes the
  returned provider context current. Audit failure publishes a typed failed
  phase and makes no provider call. After the audit await, the task rechecks its
  generations under the lifecycle lock immediately before polling
  `provider.open`; a concurrent close therefore prevents a delayed audit from
  launching the provider.
- `Agent::attachment_phase` exposes a bounded read-only projection:
  `absent`, `starting`, `attached`, or `failed { code }`. The phase grants no
  scheduling or cleanup authority.
- Existing queue, remove, reorder, receipt recovery, and close methods remain on
  `Agent`. There is no `PreparedAgent` scheduler and no gateway admission
  repository.

The current `Agent::new` callers are updated to prepare, start attachment, and
await its join handle where they require the old ready-on-return behavior. This
is one current contract, not a compatibility wrapper.

## One lifecycle admission gate

`SessionLifecycle` remains the only authority for work generations, attachment
generations, active work, stop, cleanup, and reusable reopening. Its existing
state lock also owns the attachment phase and outstanding authorization. It
performs the attachment-phase check and WorkPermit allocation or waiting-work
activation in one critical section; callers never check a phase and then ask a
second authority for work.

The delivery contracts are:

- direct `Agent::invoke` requires `attached` in the current attachment and work
  generations. `absent`, `starting`, and `failed` return a typed
  `AttachmentUnavailable` before a WorkPermit, hooks, input persistence, or
  provider operation is admitted;
- ordinary queue and boundary-steering queue may acquire bounded waiting permits
  while `absent` or `starting`. The runner can change such a permit to active and
  bind it to the current attachment only through the same lifecycle lock, when
  the attachment is `attached` and both generations still agree. The scheduler
  selects its durable queue entry only after that activation; it does not perform
  a separate phase check;
- native steering and every provider control require the attached current
  generation when their control permit is allocated. They return
  `AttachmentUnavailable` without a provider call while absent or starting, and
  a typed startup failure after a failed attempt; and
- attachment success publishes `attached` under this lock before waking the
  runner. Close/stop increments the work generation, invalidates outstanding
  attachment authorization, and fences active admission under this lock before
  any audit, storage, startup, or cleanup await.

This gate prevents a direct call or control from entering the old
`ProviderSession` while startup is absent, and prevents a queued permit from
being activated by a stale attachment after close.

## Stored representation

`SessionSnapshot` currently requires `provider_session_id`, although a new
prepared Agent has no provider context. Replace that required scalar with an
explicit provider-context value:

```text
ProviderContext::Absent
ProviderContext::Recorded(ExecutionSessionId)
```

`Absent` is the durable state before the first successful attachment.
`Recorded` is the exact context identity a later attachment must restore. It
does not claim that a provider is live in this process. The live lifecycle owns
the distinct `absent` / `starting` / `attached` phases. No placeholder provider
ID is fabricated.

Preparation loads the snapshot, validates the provider identity and every
scheduling relationship, and creates an empty `Absent` snapshot for a new
session. Attachment opens with the recorded ID when present. Before enabling
the runner, it verifies the returned identity, changes the snapshot to
`Recorded`, and durably saves it. A save failure closes the opened provider and
keeps dispatch fenced; uncertain cleanup retains the storage lease and capacity.

Storage validation accepts queued pre-attachment records only when they contain
no provider observation, provider report, permission, or provider-session
correlation. Those facts remain invalid without a recorded provider context.
Restored pending or running work still follows the existing rule: without its
original live owner it is `SubmissionUnresolved` and is never replayed merely
because attachment later succeeds.

## Authoritative input validation

Early admission means that the SDK owns an attributed input. It does not mean
that a model, provider process, or connected agent has accepted that input.
Before storage, the prepared Agent validates only facts for which it is already
the authority: execution identity and retry equality, verified actor, submission
mode, actual retained message bytes, image-reference shape, queue capacity, and
the lifecycle fence. The byte bound is measured from the value retained in the
snapshot; it is not a token estimate.

Two limits prevent failed or cancelled submissions from bypassing the live
64-item queue bound: a snapshot may retain at most 1,024 invocation records and
at most 128 MiB of cumulative attributed input data. The input budget counts
UTF-8 message text, image reference fields, linked paths, identities, and actor
fields from their validated values; image blobs remain in their separate
attachment store. Settled, refused, and cancelled records consume both limits
because they remain durable evidence. Reaching either limit returns a typed
history-capacity refusal before a new identity is saved. There is no implicit
pruning or deletion of handed or settled evidence.

`ExecutionRequest` still contains the caller's context estimate and output
reservation because they are part of stable retry identity. The server currently
constructs its conservative full-context reservation from
`Agent::capabilities().limits()`. To keep that construction available before
open, `AgentProvider` gains one read-only accessor for the
`EffectiveCapabilities` it already constructs and retains. Claude, Codex, and
Opencode providers already hold this exact value beside their configuration and
clone it into `ProviderSession` during `open`; no second policy value or gateway
capability source is introduced. `Agent::capabilities` reads the retained
configured value. Attachment verifies that the opened session reports the same
value before enabling dispatch.

All acceptance that depends on those capabilities remains at the existing
`ProviderSession::validate` boundary after attachment: modality and image limits,
estimated input plus reserved output tokens, the connected agent's negotiated
image answer, and ACP profile/frame validation. The runner performs that check
before provider entry. A refusal is saved as a typed pre-dispatch settlement on
the admitted invocation and resolves its receipt; it is not rewritten as a
startup failure. A reconnect therefore sees the original input, actor, admission,
and later refusal without implying provider delivery.

The execution audit is supplied independently to `Agent::prepare`; it is a
caller-owned application port, not something obtained by opening a provider.
Composition passes the same `Arc<dyn ExecutionAudit>` already supplied to ACP
provider construction, so provider permission evidence and Agent admission,
cancellation, and failure evidence reach one sink without a global lookup or a
new policy mechanism.

## Durable queue admission

The current queue path deliberately can own work even when
`record_queue_admission` returns a storage error: `begin_with_scheduling` has
already saved the full input, actor, mode, and first scheduling edge; the live
queue and receipt then exist; and the runner must save retained queue evidence
before provider entry. Replacing this with a second atomic admission algorithm
would widen #92 and alter an existing recovery contract without being necessary
for pre-open ownership.

Instead, the enqueue result makes that ownership explicit. It returns the stable
receipt together with a typed admission-evidence acknowledgement. A fully
acknowledged admission and an owned admission whose audit/storage acknowledgement
failed are different results, and both retain the same recoverable receipt. A
plain error is reserved for a request that the Agent proves it did not take, or
for `SubmissionUnresolved` when reconciliation cannot prove ownership. The
gateway projects the owned input from the receipt/snapshot even when it reports
the evidence failure; it never converts an owned result into an unowned transport
error.

The admission audit is added at the existing queue-admission evidence boundary
and names the session, execution, actor, mode, before/after scheduling stage, and
cause. Audit or storage acknowledgement failure leaves the Agent as owner and is
retained as a typed evidence failure. Before attachment, the runner gate prevents
provider entry. After attachment, existing retained-evidence reconciliation must
complete before selection and provider entry. If audit delivery remains failed,
the invocation is durably settled as an admission-audit failure without a
provider call; the receipt remains recoverable. This preserves the present queue
and receipt authority rather than creating a second transaction protocol.

This same scheduler lock owns complete reorder, removal, and selection. A queue
containing A and B cannot be divided between layers, so `reorder([B, A])` is
atomic against dispatch exactly as it is after attachment. The 64-input queue
bound and retained snapshot limits apply while absent and starting. Provider
controls return a typed unavailable outcome without consuming either normal
input capacity or their bounded control capacity.

Native steering remains distinct. Before attachment there can be no active
provider execution, so steering intent enters the existing priority boundary
queue. After attachment, an acknowledged native injection remains a provider
effect even if the following persistence write fails; that error is retained as
an uncertain delivery/evidence failure and is never relabelled as local
cancellation or queued work.

## Startup, close, and audit ordering

`start_attachment` marks `starting` and installs its retained task synchronously,
before `provider.open` can be polled. `Agent::close` and automatic stop
synchronously fence the work generation and record the first cancellation cause
before they await audit, storage, provider startup, or cleanup. Therefore caller
loss or audit failure cannot leave the runner eligible to dispatch.

Close cancels every waiting receipt through the same scheduler whether the
attachment is absent, starting, or attached. Each transition retains its exact
input, target, prior stage, cause, and known caller. Every audit record gets its
own bounded delivery attempt. Audit failure remains visible and prevents a
successful audited close result, while cancellation fencing and necessary
cleanup continue.

If close wins before `provider.open` is polled, no provider starts. If it wins
while open is pending, the Agent-owned attachment task continues awaiting the
operation; it does not drop the future, because initialization may continue
independently.
An eventual session is closed without enabling the runner. An initialization
error with cleanup ownership remains attached to the Agent until retry confirms
release. If open and close are ready in one poll, close is checked before
publishing `attached` or waking dispatch. Capacity and the storage lease remain
owned until cleanup is confirmed.

Local cancellation, provider acknowledgement, provider cleanup, storage
acknowledgement, and audit acknowledgement remain separate facts. No diagnostic
string decides any of them.

Close remains a reusable SDK operation. A close that reaches confirmed cleanup
and satisfies the existing `maybe_reopen` conditions makes the lifecycle open
for a future attachment, but it does not reuse the old attachment generation.
An authorization issued before that close is permanently stale, whether its
caller delayed `start_attachment`, lost its task, or uses it after
`maybe_reopen`. A fresh explicit authorization can be issued only after the old
attachment task is terminal, cleanup is confirmed, required audit succeeded,
and all old active ownership has retired. Starting it creates a new attachment
generation. Explicitly cancelled queued work keeps its terminal scheduling
evidence and cannot be activated in that generation; only waiting work that the
existing automatic-recovery rules deliberately preserved in the current work
generation may run after a newly authorized recovery attachment.

This proposal does not change `ExecutionReport` or introduce a second policy-stop
authority. Its existing provider-result versus local-cancellation exclusivity
continues to apply to invocations. Closing during attachment has no dispatched
invocation to settle: its local startup stop is retained in the attachment and
session lifecycle, while an eventual opened context contributes only its separate
cleanup report. Any future policy that must preserve both a provider result and
a captured local policy stop depends on the execution-lifecycle work tracked in
#132/#136 and is outside #92.

## Gateway and product projection

The gateway conversation slot holds the prepared Agent immediately. `read`,
`submit`, remove, reorder, and close no longer call a resolver that waits for
provider startup. The slot's retained task alone waits for runtime readiness and
calls `Agent::start_attachment` with the generation-bound authorization captured
for that slot; the Agent then owns the attachment task even if the slot task or
its join handle is dropped. Closing the slot while readiness is pending
invalidates that authorization, so the delayed task cannot revive it.

`ConversationView` adds a lifecycle projection with a closed phase and a typed
startup failure code plus bounded display text. While absent or starting,
messages admitted to the one SDK queue appear as pending and the conversation
says the agent is starting; the panel does not claim the model is thinking.
After attachment, the same projection and receipt identities continue. A late
startup failure settles affected SDK receipts and scheduling evidence, so a
reconnect rebuilds failed messages from the authoritative snapshot. Repeated
reads cannot revive them.

`conversation.create` and queued send receipts use the ordinary bounded product
command deadline. `conversation.read` likewise returns the prepared projection
without waiting for attachment. Provider startup budgets remain server/SDK
lifecycle budgets and no longer determine client command timeouts. The protocol
schema remains the single generated source; no version bump or legacy path is
introduced.

The terminal projection work in #121 continues to consume SDK terminal evidence.
This design adds no gateway terminal ledger and reserves no competing projection
authority.

## Enforcers and affected ownership

| Contract | Enforcer | Primary modules |
| --- | --- | --- |
| One queue before and after startup | One `Agent::Inner::scheduler` and scheduler lock | SDK `agents/agent.rs`, `agents/scheduling.rs` |
| No active/direct/control admission before attachment | One `SessionLifecycle` lock checks attachment/work generations while allocating or activating permits | SDK `agents/lifecycle.rs`, agent and scheduling entry points |
| No fabricated provider context | `ProviderContext` construction and snapshot validation | SDK `sessions/storage.rs`, `sessions/manager.rs`, validation and storage adapters |
| Pre-open ownership bounds | Agent identity/retry checks, retained-byte bound, queue/lifecycle permits | SDK request, scheduling, session manager |
| Configured token reservation source | Existing provider `EffectiveCapabilities` exposed read-only before open | SDK provider port and ACP provider factories |
| Model/provider acceptance | Existing attached `ProviderSession::validate`, saved typed pre-dispatch settlement | SDK provider session, ACP binding/profile, scheduling |
| Durable audited admission before dispatch | Agent audit port plus existing retained-evidence reconciliation | SDK scheduling, session manager, execution audit |
| Close fences and invalidates delayed startup before awaits | `SessionLifecycle` state transition under its mutex | SDK lifecycle and close coordination |
| Reusable close cannot revive stale work | Fresh generation-bound attachment authorization after confirmed cleanup and owner retirement | SDK lifecycle, attachment task, scheduling activation |
| Caller loss retains work/startup | Existing supervised submission/close tasks plus one Agent-owned attachment task | SDK scheduling and agent coordination |
| Starting and late failure survive socket loss | SDK snapshot plus conversation replacement projection | server conversation service/view/projection, product protocol/client/panel |

Public SDK documentation must describe preparation, attachment phases, admission
and close behavior before attachment, and resource ownership on failed attach.
The SDK module diagram changes from initialization-before-Agent to one Agent with
a two-stage attachment. The gateway chat guide and repository architecture map
change only when implementation makes this the current contract.

## Deterministic verification

Tests gate storage load/save, admission audit, runtime readiness, provider open,
attachment publication, queue selection, close audit, and cleanup. They cover:

- create and multiple sends returning while readiness and provider open are held;
- direct invocation returning `AttachmentUnavailable` while absent, starting,
  and failed without hooks, persistence, WorkPermit, or provider calls;
- queued, boundary-steering, native-steering, permission/control, and direct
  modes at every attachment phase, including the exact generation admitted;
- complete reorder and remove racing selection while absent and starting;
- exact queue capacity plus attachment and close progress under saturated input
  and control capacity;
- caller loss during admission, audit, open, and close;
- audit refusal during admission and during bulk startup cancellation;
- storage failure before admission, uncertain queue-admission save, and attach-ID
  publication failure;
- close before first open poll, while pending, after provider result, and both
  open and close ready in one poll;
- close while attachment-attempt audit is pending, proving that audit completion
  cannot launch the fenced generation;
- an authorization captured before close being refused after confirmed
  `maybe_reopen`, plus a fresh authorization succeeding after cleanup and
  old-owner retirement;
- caller loss before attachment start, after the Agent-owned task is installed,
  and while its join handle waits;
- eventual provider cleanup after a locally cancelled startup, including audit
  failure and unconfirmed cleanup retry;
- restoration of `ProviderContext::Absent` and `Recorded`, including impossible
  provider evidence while absent;
- unresolved restored work staying unresolved after attach, with no provider
  call; and
- disconnect/reconnect during starting and after a late startup failure.

Each test asserts authoritative snapshot, live queue, provider call count,
receipt result, audit record, attachment phase, and retained cleanup owner as
applicable. Tests use gates and channels rather than elapsed sleeps.

## Findings disposition from design review round 1

1. **Split queue breaks atomic complete reorder — accepted.** The gateway
   admission repository is removed. Prepared and attached work use the same SDK
   scheduler, queue lock, and persistence history.
2. **An enqueue or native-steering error can follow transferred ownership —
   accepted.** The existing queue algorithm remains the single authority and
   returns its stable receipt with a typed evidence acknowledgement when it owns
   work. Native steering retains provider acknowledgement separately from a
   later persistence failure; neither path uses `Ok`/`Err` alone as evidence of
   ownership or delivery.
3. **Cancellation must fence dispatch before audit or cleanup awaits — accepted.**
   Close first changes the lifecycle generation and records its cause under the
   lifecycle mutex. Audit, startup completion, provider close, storage retention,
   and cleanup follow as separately reported facts. Audit failure cannot reopen
   selection or permit dispatch, and local cancellation never claims provider
   acknowledgement or confirmed cleanup.

## Findings disposition from design review round 2

1. **Direct invocation before attachment was undefined — accepted.** Direct
   invocation, native steering, and controls now require the attached current
   generation at the single lifecycle admission gate. Queue and boundary
   steering retain bounded waiting ownership, and activation uses that same
   gate without a phase-check/admission race.
2. **Reusable close could let delayed startup revive an old generation —
   accepted.** Attachment requires a single-use authorization bound to both
   lifecycle generations. Close invalidates it synchronously. Reopening requires
   fresh explicit authority after the prior task, cleanup, audit, and active
   owners resolve, and cancelled queued work cannot cross that boundary.

No source or protocol implementation should begin until this replacement design
passes the next design review.
