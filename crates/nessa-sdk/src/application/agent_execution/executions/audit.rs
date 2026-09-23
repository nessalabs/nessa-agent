//! Mandatory execution lifecycle evidence, independent of live event consumers.
#![deny(missing_docs)]

use crate::application::agent_execution::agents::{AgentError, AgentFuture};
use crate::application::agent_execution::permissions::{
    ActionContext, CancellationOrigin, PermissionAnswerRecord, PermissionCancellation,
    ReviewDeclineRecord,
};
use crate::domain::agent_execution::executions::{
    ExecutionId, InvocationKind, InvocationStage, QueueOrderChange, SchedulingCause,
    SchedulingInitiator, SchedulingTransition, SchedulingTransitionError, SubmissionMode,
};
use crate::domain::agent_execution::sessions::{
    AttachmentCause, ExecutionFinish, SessionClosure, SessionId,
};

/// Audited lifecycle stage of one provider attachment attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentAuditStage {
    /// No attachment attempt is authorized.
    Absent,
    /// One generation-bound authorization is waiting to start.
    Waiting,
    /// The Agent-owned provider-open task is running.
    Starting,
    /// Provider context is durably saved but is not dispatchable until this evidence is acknowledged.
    ContextPublished,
    /// The provider context was durably published and is usable.
    Attached,
    /// The attempt ended without a usable provider context.
    Failed,
}

/// Why an attachment lifecycle transition occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentAuditCause {
    /// The authorized attempt was started for its validated domain cause.
    Started(AttachmentCause),
    /// Its unused authorization owner was dropped.
    AuthorizationAbandoned,
    /// Session closure cancelled an authorized or starting attachment.
    Closed,
    /// Provider startup completed and durable context publication succeeded.
    Published,
    /// Audit, provider startup, storage publication, or cleanup failed.
    Failed,
}

/// Attributed before/after evidence for one attachment lifecycle transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentAuditRecord {
    session_id: SessionId,
    generation: u64,
    before: AttachmentAuditStage,
    after: AttachmentAuditStage,
    cause: AttachmentAuditCause,
    actor: Option<ActionContext>,
}
impl AttachmentAuditRecord {
    /// Construct exact attachment transition evidence without performing effects.
    pub fn new(
        session_id: SessionId,
        generation: u64,
        before: AttachmentAuditStage,
        after: AttachmentAuditStage,
        cause: AttachmentAuditCause,
        actor: Option<ActionContext>,
    ) -> Self {
        Self {
            session_id,
            generation,
            before,
            after,
            cause,
            actor,
        }
    }
    /// Local session whose attachment lifecycle changed.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    /// Attachment generation affected by this transition.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Stage before the transition.
    pub fn before(&self) -> AttachmentAuditStage {
        self.before
    }
    /// Stage after the transition.
    pub fn after(&self) -> AttachmentAuditStage {
        self.after
    }
    /// Typed transition cause.
    pub fn cause(&self) -> AttachmentAuditCause {
        self.cause
    }
    /// Verified caller for explicit causes, absent for automatic ones.
    pub fn actor(&self) -> Option<&ActionContext> {
        self.actor.as_ref()
    }
}

/// Audited ownership stage for a submitted input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionAuditStage {
    /// The SDK has not accepted responsibility for this input.
    Unowned,
    /// The SDK owns the input and its stable settlement receipt.
    Owned,
}

/// Why input ownership changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionAuditCause {
    /// A verified caller submitted the input through the selected delivery mode.
    Submitted,
}

/// Evidence that an admitted queued input settled before provider dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueSettlementRecord {
    session_id: SessionId,
    execution_id: ExecutionId,
    transition: SchedulingTransition,
    submitted_by: ActionContext,
    initiated_by: Option<ActionContext>,
}
impl QueueSettlementRecord {
    /// Describe automatic attachment failure for one caller-attributed queued input.
    /// `session_id` is the local owner and `execution_id` is the stable queued input.
    /// `kind` and `target` must identify ordinary queued work without a target, or
    /// boundary steering with its correlated target. `actor` is the original
    /// verified submitter and is not represented as the initiator of this automatic failure.
    ///
    /// # Errors
    /// Returns [`SchedulingTransitionError`] when `kind` and `target` contradict.
    pub fn automatic_attachment_failed(
        session_id: SessionId,
        execution_id: ExecutionId,
        kind: InvocationKind,
        target: Option<ExecutionId>,
        actor: ActionContext,
    ) -> Result<Self, SchedulingTransitionError> {
        let transition = SchedulingTransition::new(
            kind,
            target,
            Some(InvocationStage::Queued),
            InvocationStage::Settled,
            SchedulingCause::DispatchFailed,
            SchedulingInitiator::Automatic,
        )?;
        Ok(Self {
            session_id,
            execution_id,
            transition,
            submitted_by: actor,
            initiated_by: None,
        })
    }
    /// Describe cancellation of caller-attributed queued input.
    /// `session_id` is the local owner, `execution_id` is the stable queued input,
    /// and `submitted_by` is its original verified caller. `kind` and `target`
    /// retain its admitted delivery intent.
    /// `cause` must be `SessionClosed` with a verified `initiated_by` caller, or
    /// `RunnerStopped` without one. The domain transition rejects contradictory
    /// stage, target, cause, and initiator combinations.
    ///
    /// # Errors
    /// Returns [`SchedulingTransitionError`] when any supplied fact contradicts
    /// the legal queued cancellation transition.
    pub fn cancelled(
        session_id: SessionId,
        execution_id: ExecutionId,
        kind: InvocationKind,
        target: Option<ExecutionId>,
        cause: SchedulingCause,
        submitted_by: ActionContext,
        initiated_by: Option<ActionContext>,
    ) -> Result<Self, SchedulingTransitionError> {
        let initiator = if initiated_by.is_some() {
            SchedulingInitiator::Caller
        } else {
            SchedulingInitiator::Automatic
        };
        let transition = SchedulingTransition::new(
            kind,
            target,
            Some(InvocationStage::Queued),
            InvocationStage::Cancelled,
            cause,
            initiator,
        )?;
        Ok(Self {
            session_id,
            execution_id,
            transition,
            submitted_by,
            initiated_by,
        })
    }
    /// Local session that owned the queued input.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    /// Stable identity of the queued input.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Active execution targeted by boundary steering, when applicable.
    pub fn target(&self) -> Option<&ExecutionId> {
        self.transition.target()
    }
    /// Delivery mode retained at admission.
    pub fn mode(&self) -> SubmissionMode {
        match self.transition.kind() {
            InvocationKind::Queued => SubmissionMode::Queued,
            InvocationKind::Steering => SubmissionMode::BoundarySteering,
        }
    }
    /// Original verified caller, distinct from the automatic settlement cause.
    pub fn submitted_by(&self) -> &ActionContext {
        &self.submitted_by
    }
    /// Verified caller that caused this transition, absent for automatic causes.
    pub fn initiated_by(&self) -> Option<&ActionContext> {
        self.initiated_by.as_ref()
    }
    /// Stage before the local transition.
    pub fn before(&self) -> Option<InvocationStage> {
        self.transition.before()
    }
    /// Stage after the local transition.
    pub fn after(&self) -> InvocationStage {
        self.transition.stage()
    }
    /// Exact lifecycle cause of settlement or cancellation.
    pub fn cause(&self) -> SchedulingCause {
        self.transition.cause()
    }
    /// Initiator classification, kept separate from original submitter attribution.
    pub fn initiator(&self) -> SchedulingInitiator {
        self.transition.initiator()
    }
}

/// Audited delivery stage for native steering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteeringAuditStage {
    /// Input is durably accepted locally but no provider effect is confirmed.
    Pending,
    /// The provider confirmed injection into the correlated active execution.
    Injected,
}

/// Why native steering delivery changed stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteeringAuditCause {
    /// The provider acknowledged accepting the input into the active execution.
    ProviderAcknowledged,
}

/// Attributed provider acknowledgement kept distinct from later evidence writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteeringAcknowledgementRecord {
    session_id: SessionId,
    execution_id: ExecutionId,
    target: ExecutionId,
    actor: ActionContext,
    before: SteeringAuditStage,
    after: SteeringAuditStage,
    cause: SteeringAuditCause,
}
impl SteeringAcknowledgementRecord {
    /// Describe a provider-confirmed injection for one caller-attributed input.
    pub fn provider_acknowledged(
        session_id: SessionId,
        execution_id: ExecutionId,
        target: ExecutionId,
        actor: ActionContext,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            target,
            actor,
            before: SteeringAuditStage::Pending,
            after: SteeringAuditStage::Injected,
            cause: SteeringAuditCause::ProviderAcknowledged,
        }
    }
    /// Local session whose provider accepted the input.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    /// Stable identity of the steering input.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Active execution that accepted the steering input.
    pub fn target(&self) -> &ExecutionId {
        &self.target
    }
    /// Verified caller attribution.
    pub fn actor(&self) -> &ActionContext {
        &self.actor
    }
    /// Delivery stage before acknowledgement.
    pub fn before(&self) -> SteeringAuditStage {
        self.before
    }
    /// Delivery stage after acknowledgement.
    pub fn after(&self) -> SteeringAuditStage {
        self.after
    }
    /// Typed delivery cause.
    pub fn cause(&self) -> SteeringAuditCause {
        self.cause
    }
}

/// Attributed evidence that the SDK accepted responsibility for one input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueAdmissionRecord {
    session_id: SessionId,
    execution_id: ExecutionId,
    mode: SubmissionMode,
    actor: ActionContext,
    before: AdmissionAuditStage,
    after: AdmissionAuditStage,
    cause: AdmissionAuditCause,
}
impl QueueAdmissionRecord {
    /// Describe caller-attributed ownership after the receipt has been installed.
    pub fn submitted(
        session_id: SessionId,
        execution_id: ExecutionId,
        mode: SubmissionMode,
        actor: ActionContext,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            mode,
            actor,
            before: AdmissionAuditStage::Unowned,
            after: AdmissionAuditStage::Owned,
            cause: AdmissionAuditCause::Submitted,
        }
    }
    /// Local session that owns the receipt.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    /// Stable execution correlation supplied by the caller.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Delivery mode whose immutable intent was saved.
    pub fn mode(&self) -> SubmissionMode {
        self.mode
    }
    /// Verified caller attribution.
    pub fn actor(&self) -> &ActionContext {
        &self.actor
    }
    /// Ownership before the transition.
    pub fn before(&self) -> AdmissionAuditStage {
        self.before
    }
    /// Ownership after the transition.
    pub fn after(&self) -> AdmissionAuditStage {
        self.after
    }
    /// Typed transition cause.
    pub fn cause(&self) -> AdmissionAuditCause {
        self.cause
    }
}

/// Why a pending dispatch order was replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueOrderCause {
    /// A verified caller requested the complete replacement order.
    CallerRequested,
}

/// Audited selection of one pending-order replacement before local application.
/// The session snapshot remains authoritative for whether the selected order was
/// subsequently applied and saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueOrderRecord {
    session_id: SessionId,
    change: QueueOrderChange,
    actor: ActionContext,
    cause: QueueOrderCause,
}
impl QueueOrderRecord {
    /// Pair the owning `session_id`, validated complete `change`, and verified `actor`.
    /// Construction performs no I/O and does not mutate the live queue.
    pub fn caller_requested(
        session_id: SessionId,
        change: QueueOrderChange,
        actor: ActionContext,
    ) -> Self {
        Self {
            session_id,
            change,
            actor,
            cause: QueueOrderCause::CallerRequested,
        }
    }
    /// Local session whose pending order is affected.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    /// Complete validated before/after order and immutable priorities.
    pub fn change(&self) -> &QueueOrderChange {
        &self.change
    }
    /// Host-verified caller attribution.
    pub fn actor(&self) -> &ActionContext {
        &self.actor
    }
    /// Causal reason for the order transition.
    pub fn cause(&self) -> QueueOrderCause {
        self.cause
    }
}

/// The local live aggregate's closure, paired with its known initiator.
/// This evidence does not claim provider deletion or completed process cleanup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionClosureRecord {
    closure: SessionClosure,
    origin: CancellationOrigin,
}
impl SessionClosureRecord {
    /// Pairs the domain's once-only `closure` evidence with the known `origin`.
    /// The host supplies verified caller attribution for explicit closure; adapters
    /// label provider and automatic causes. Contradictory cause/origin pairs return
    /// InvalidInput. Construction performs no I/O.
    pub fn new(closure: SessionClosure, origin: CancellationOrigin) -> Result<Self, AgentError> {
        origin.validate_reason(closure.reason())?;
        Ok(Self { closure, origin })
    }
    /// Session identity, optional active execution, and causal lifecycle reason.
    pub fn closure(&self) -> &SessionClosure {
        &self.closure
    }
    /// Verified client attribution or the honestly labelled automatic origin.
    pub fn origin(&self) -> &CancellationOrigin {
        &self.origin
    }
}

/// Evidence supplied to the mandatory audit sink in causal order.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "execution evidence must be recorded or its delivery failure reported"]
pub enum ExecutionAuditRecord {
    /// Attachment authorization, start, publication, failure, or abandonment.
    Attachment(AttachmentAuditRecord),
    /// Caller-attributed transfer of one input into SDK-owned scheduling.
    QueueAdmitted(QueueAdmissionRecord),
    /// Pre-dispatch settlement or cancellation of caller-attributed queued input.
    QueueSettled(QueueSettlementRecord),
    /// Provider acknowledgement of native steering delivery.
    SteeringAcknowledged(SteeringAcknowledgementRecord),
    /// Caller-attributed order selected and acknowledged before local application.
    QueueReordered(QueueOrderRecord),
    /// Once-only release of an active execution, including runs with no permissions.
    /// The runtime initiates this release after observing a terminal result; explicit
    /// shutdown attribution remains on the preceding SessionClosed record.
    Finished(ExecutionFinish),
    /// Once-only closure of a live aggregate, including idle contexts.
    SessionClosed(SessionClosureRecord),
    /// Once-only cancellation with its original lifecycle cause and initiator.
    Cancelled(PermissionCancellation),
    /// Selection before effects, followed by a separate wire delivery observation.
    Answered(PermissionAnswerRecord),
    /// A review refused by this binding before any host was offered it, with the
    /// same separation of local decision from observed delivery.
    ReviewDeclined(ReviewDeclineRecord),
}

/// Required audit boundary, independent of bounded UI streams and caller waits.
/// Composition supplies controlled storage and documents its durability contract.
/// Implementations assign record identities and observation/commit times through
/// injected infrastructure. Exact inputs must not be copied into general logs.
pub trait ExecutionAudit: Send + Sync {
    /// Accept `record` under the sink's durability contract before returning success.
    /// Errors or bounded adapter timeouts prevent successful audited acknowledgements
    /// but must not prevent necessary cleanup. Delivery observations never establish
    /// provider acknowledgement or tool execution. Record every call in order.
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()>;
}
