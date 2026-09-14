# Permission review, attribution, and audit

The required `ExecutionAudit` port lives in `application::agent_execution::executions`.
Its records cover permission cancellation, answer selection/delivery, and live
session closure. A `SessionClosed` record retains the domain's open-to-closed
transition, session identity, optional active execution, lifecycle reason, and
known initiator. It is emitted once per live aggregate even when idle. Restoring
that provider context creates a fresh aggregate with its own closure transition.
Closure evidence describes local state; confirmed process cleanup remains a
separate result. Sink failure is reported while cleanup and remaining permission
audit attempts still run.

## Cancellation audit evidence

Every admitted review cancelled by provider withdrawal, explicit session close,
execution completion, deadline, execution failure, or dropped handles/consumers
produces an immutable `PermissionCancellation`. It contains session, execution,
permission and tool identities, offered options, the original review input, the
cancelled domain state with its first lifecycle reason, and its initiator.
`Agent::close(action)` and explicit `Agent::cancel_permission(request)` require
host-verified `ActionContext`; provider and
runtime causes remain explicit rather than inventing a user identity.
An automatic guard can supply `PermissionCancellationReason::custom` with a
validated `CustomPermissionCancellationReason::new(code, explanation)`, plus its
own principal/surface/request attribution. The code is limited to 128 UTF-8 bytes
and explanation to 1,024 bytes; both must be nonblank. The reason explains the
decision and never acts as permission to bypass host authorization.
`PermissionCancellationReason::view` borrows the cause and any custom payload.
`PermissionRequest::state` returns a borrowed `PermissionStateView`; its selected
option, decision, and cancellation cause cannot be changed independently of the
request's validated transitions.

The required `ExecutionAudit` port receives records before their UI
projections and before successful completion is reported. A dropped/full UI queue
cannot prevent audit capture of the rest of a cancellation batch. Sink rejection
or a bounded recording timeout reports `AgentError::AuditFailure`; process
cleanup still runs. If cleanup also cannot be verified, `AuditAndCleanupFailure`
reports both failures instead of hiding either. Repeated close does not overwrite reasons or duplicate records.
The SDK uses diagnostics for delivery failure even if callers have dropped all
handles; diagnostics themselves are not the audit store.

Composition must provide an adapter with an explicit storage/durability contract.
The adapter owns record IDs and timestamps through its infrastructure dependencies;
these are observation/commit times, not an inferred time of provider effects.
Tests record in memory. The example's tracing adapter is explicitly non-durable.
A production audit database and transactional outbox are not implemented by this
SDK, and it does not claim crash-safe recording of a transition before a sink
accepts it. Never put raw review input in general diagnostic logs.

This evidence describes a **local permission cancellation**. It does not prove
that a cancellation response reached the provider, that a tool was stopped, or
that its effects were rolled back. Rejected/unadmitted protocol messages are
protocol failures, not invented domain permission records. Confirmed process
cleanup remains a separate outcome.

Standalone cancellation cannot report `SessionClosed`, `SessionFailed`,
`EventConsumerDropped`, or `SessionHandlesDropped` while the aggregate is open. Those causes belong to `close`,
which records the session transition and cancels its pending reviews together.
Standalone cancellation also rejects `ExecutionFinished` and `ExecutionFailed`,
including empty batches. `finish_execution` performs the correlated execution
transition and cancels pending reviews atomically; an active failure close can
retain that failure correlation. A cancelled provider outcome waits for session
teardown before publishing confirmed cancellation. Invalid cancellation causes
preserve the existing requests and evidence.

Domain requests and options expose retained payload measurements. The session's
`permission_payload_bytes` includes pending request allocations and both map/set
identity copies; `permission_payload_bytes_after` predicts admission without
mutation. Resolving a request releases its pending payload and map key, while the
seen identity remains until execution completion. The application applies its own
count and byte limits using these domain measurements; fixed collection overhead
and host review-input allocations are accounted for separately.

## Decisions and review attribution

`PermissionDecision` separates `PermissionEffect::{Allow, Deny}` from
`PermissionScope`: the exact request, a named session within a named application,
or a named application boundary. Session/application identities are host-owned,
not socket identities. The owning `PermissionRequest` retains the exact tool identity; a broader scope does not mean every tool or resource.
`PermissionOfferPolicy` permits exact effect/scope pairs, and `PermissionOptions`
filters choices against those pairs. Both retain compact immutable collections;
policy construction and batch filtering use sets for lookup while preserving the
configured policy order and offered-choice order. Configuration grants no authority.
`PermissionAnswer` selects an exact offered `option_id`.

Every answer requires an immutable application `ApprovalAttribution`:

- `ActionContext` identifies the acting principal, verified surface, and action
  request ID. Hosts verify identity/access before constructing this context;
  validation of strings alone proves no authority. Credentials stay out of it.
- `ApprovalBasis::Explicit` records a direct answer. `Mode` records the effective
  mode name and immutable configuration revision. `Rule` identifies the exact rule
  revision and its original granting actor/action. Automatic answers use their own
  host actor, preserving the human grantor separately. Hosts retain referenced
  configuration/rule revisions; these values do not implement a policy evaluator.
- `answer_permission` returns `PermissionResolution` after a successful protocol
  write, containing the resolving controller’s provider-session identity, answered
  domain request, original review input, and supplied attribution.
  Allowances and denials have the same attribution requirements. Pending or
  cancelled requests cannot form a resolution. Invalid/stale answers return errors.

Each attribution identity, mode name, and rule/configuration revision is nonblank
and limited to 256 UTF-8 bytes (`ActionContext::MAX_IDENTITY_BYTES`). Constructors
reject oversized values before admission or persistence, preserve exact text, and
compact caller allocations before cloning evidence. Snapshot restoration applies
the same constructors to invocation and scheduling actors and cancellation origins;
invalid saved attribution is rejected without rewriting history.

The required `ExecutionAudit` receives `ExecutionAuditRecord::Answered`
evidence for allowances and denials. Each immutable `PermissionAnswerRecord`
retains the exact `PermissionResolution` and its `PermissionAnswerDelivery` stage.
Its provider-session identity is derived from the immutable resolution; an adapter
cannot attach a different session to an existing answer. The controller supplies
that identity when it resolves the review, so identical execution or review IDs
in different provider sessions remain distinct. `Selected` is recorded before attempting the
wire response. `Written` records a completed local write; `Failed(error)` records
an uncertain delivery attempt. Neither stage proves provider acknowledgement or
a tool effect. The input, offered choices, selected effect/scope, and actor remain
available through the resolution. Cancellation records use the same audit port.

At the public Agent boundary, polling an answer or cancellation starts supervised
control handling. Dropping the caller's future does not abandon that handling.
If the provider returns uncertain cleanup, including a wrapped cleanup failure,
Agent blocks new invocation, queue, and steering admission until an explicit close
confirms cleanup. Errors with confirmed cleanup do not leave that barrier raised.
Provider adapters still own permission correlation, wire delivery, and mandatory audit.

Once an answer or cancellation command is admitted to the worker queue, the worker retains and audits it even if the caller
stops waiting. A failed or timed-out selection audit prevents wire delivery. A
failed delivery audit prevents a successful answer acknowledgement. Both paths
report `AuditFailure` and continue process cleanup. If the wire write and its
delivery audit both fail, `PermissionAnswerDeliveryAndAuditFailure` retains the
original write error and any additional cleanup failure. Later cleanup cannot relabel
an answered review as cancelled. The audit sink supplies durability independently
of Agent snapshots and UI readers; no production database is bundled. Do not retry
a tool from a lost answer response: `Written` is not an idempotent command receipt.
Every teardown seals command admission and drains already-admitted permission
controls before closing the live aggregate, including failure, deadline, and
consumer-loss exits. A concurrent close or fault therefore cannot replace an
accepted caller decision with automatic bulk cancellation. The sealed queue is
bounded; later requests are rejected and cannot prolong draining. Independent
control failures remain ordered in `MultipleOperationFailures`; they are distinct
from resource cleanup failures. Confirmed process termination cannot erase audit
failure or make an independent transport error imply uncertain cleanup.

The current Claude profile requires `PermissionOfferPolicy::once_only()` or a subset
of its exact-request choices. It rejects session/application configuration before
spawning. ACP persistent choices are validated structurally then omitted because
this binding has no structured scope or rule enforcement for them; it never infers
a scope from a provider label or downgrades a persistent choice to request scope.
Domain values describe scoped intent; enabling reuse requires host authorization,
domain target/scope evaluation, and application-owned storage/revocation effects.
There is no stored grant or automatic rule engine in this binding.

Regression coverage lives under `tests/infrastructure/acp/`, compiled internally for private adapter access:
[permission answers](../../tests/infrastructure/acp/contracts/permissions/answers.rs)
and [permission cancellations](../../tests/infrastructure/acp/contracts/permissions.rs).
The answer tests cover allow/deny attribution, once-only cleanup, dropped waits,
selection/delivery audit rejection, bounded sink timeouts, and failed wire writes.

Execution release also crosses the mandatory `ExecutionAudit` port as `Finished`,
even when no permission was requested. The once-only `ExecutionFinish` retains
the provider context, execution ID, and reported outcome or failed cleanup cause.
Its initiator is the runtime; explicit shutdown attribution remains on the
preceding `SessionClosed` record. Successful terminal publication waits for audit
delivery. Sink failure remains visible while resource cleanup still runs.
See [lifecycle audit tests](../../tests/infrastructure/acp/contracts/audit.rs).

Explicit review cancellation accepts a validated `Custom` reason from the
host-authorized caller or guard. Provider/runtime causes cannot be relabelled as
caller actions. The controller validates attribution before resolving a review;
restored cancellation evidence is checked against the same cause/origin rules and
against its exact preceding request, including original input and offered options.

Live decision authority belongs to the execution aggregate and its application
controller. Public permission requests expose their identities, options, and state;
callers submit answers or withdrawals through the owning session. Entity terminal
transitions are internal. Reconstructing IDs or a detached request does not resolve
an already admitted review or authorize a provider response. Restored cancellation
records are immutable evidence, not a second live permission authority. These
invariants protect one owned execution; matching identifiers do not make separately
constructed session aggregates share state or establish host authorization.
