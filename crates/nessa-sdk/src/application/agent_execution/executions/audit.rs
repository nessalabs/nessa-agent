//! Mandatory execution lifecycle evidence, independent of live event consumers.
#![deny(missing_docs)]

use super::ConfigurationControlRecord;

use crate::application::agent_execution::agents::{AgentError, AgentFuture};
use crate::application::agent_execution::permissions::{
    ActionContext, CancellationOrigin, PermissionAnswerRecord, PermissionCancellation,
    QuestionAnswerRecord, QuestionRefusalRecord, ReviewDeclineRecord,
};
use crate::application::agent_execution::providers::{ApprovalMode, ProviderIdentity};
use crate::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, InvocationKind, InvocationStage, QueueOrderChange,
    SchedulingCause, SchedulingInitiator, SchedulingTransition, SchedulingTransitionError,
    SubmissionMode,
};
use crate::domain::agent_execution::sessions::{
    AttachmentCause, ExecutionFinish, ExecutionSessionId, SessionClosure, SessionId,
};
use crate::domain::model_metadata::value_objects::EffortLevel;

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

pub use crate::application::agent_execution::agents::lifecycle::AttachmentAuditRecord;

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
    approval_mode: Option<ApprovalMode>,
    effort_level: Option<EffortLevel>,
    admission_generation: Option<String>,
    model: Option<ProviderIdentity>,
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
            approval_mode: None,
            effort_level: None,
            admission_generation: None,
            model: None,
        }
    }
    /// Attach the provider preset and Agent-instance generation that governed
    /// admission. Generation zero says attachment had not yet been verified.
    pub(crate) fn with_approval_context(
        mut self,
        approval_mode: Option<ApprovalMode>,
        admission_generation: String,
    ) -> Self {
        self.approval_mode = approval_mode;
        self.admission_generation = Some(admission_generation);
        self
    }
    /// Exact configured provider/model identity captured at input admission.
    /// This remains the admitted choice even if native configuration changes later.
    pub fn model(&self) -> Option<&ProviderIdentity> {
        self.model.as_ref()
    }
    pub(crate) fn with_model(mut self, model: ProviderIdentity) -> Self {
        self.model = Some(model);
        self
    }
    /// Attach the reasoning effort level in force at admission.
    pub(crate) fn with_effort_level(mut self, effort_level: Option<EffortLevel>) -> Self {
        self.effort_level = effort_level;
        self
    }
    /// Approval preset of the attachment that accepted this input. A live
    /// change on that attachment is recorded here, including when close
    /// detaches it before this record is written. Work accepted while nothing
    /// is attached records the binding's preset, which the next attachment
    /// opens at. A turn still queued when its attachment is replaced runs at
    /// the new attachment's preset, which this record does not name.
    pub fn approval_mode(&self) -> Option<ApprovalMode> {
        self.approval_mode
    }
    /// Reasoning effort level in force when this input was admitted
    /// ([`Agent::effort_level`](crate::application::agent_execution::agents::Agent::effort_level)).
    /// `None` means no level is sent and the agent keeps its own default. A
    /// turn still queued when its attachment is replaced starts at the new
    /// attachment's level, which this record does not name.
    pub fn effort_level(&self) -> Option<&EffortLevel> {
        self.effort_level.as_ref()
    }
    /// Agent-instance and provider-generation correlation at admission.
    pub fn admission_generation(&self) -> Option<&str> {
        self.admission_generation.as_deref()
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

/// Where a caller's live reasoning effort change stands: asked for before
/// anything is sent, then settled once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffortChangeStage {
    /// Recorded before the request is sent. Nothing has reached the agent.
    Requested,
    /// The agent reported the requested level back: it is the level in force.
    /// This verifies native effect, not durable configuration storage; a later
    /// settlement failure is returned separately by the operation.
    Applied,
    /// The connection refused the change without sending it (a turn or a
    /// permission still open, or a backend that cannot change it live): the
    /// level is unchanged and the attachment usable.
    Refused,
    /// The request failed or its answer could not be verified: the level in
    /// force is not known to have changed, and the attachment must be retired.
    Failed,
}

/// How a requested effort change settled; one of the non-requested stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffortChangeOutcome {
    /// See [`EffortChangeStage::Applied`].
    Applied,
    /// See [`EffortChangeStage::Refused`].
    Refused,
    /// See [`EffortChangeStage::Failed`].
    Failed,
}

/// One caller's live change of the reasoning effort level on one attachment
/// ([`Agent::set_effort_level`](crate::application::agent_execution::agents::Agent::set_effort_level)):
/// the source of both records that describe it, so the two always agree on
/// identity, caller and levels, and a record can only be the request or one
/// settlement of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffortLevelChange {
    session_id: SessionId,
    attachment_generation: String,
    actor: ActionContext,
    before: Option<EffortLevel>,
    after: EffortLevel,
}
impl EffortLevelChange {
    /// A change of the local session `session_id`'s attachment
    /// `attachment_generation` (Agent instance and provider generation, as on
    /// [`QueueAdmissionRecord::admission_generation`]) from `before` — `None`
    /// is the agent's own default — to `after`, asked for by `actor`.
    pub fn new(
        session_id: SessionId,
        attachment_generation: String,
        actor: ActionContext,
        before: Option<EffortLevel>,
        after: EffortLevel,
    ) -> Self {
        Self {
            session_id,
            attachment_generation,
            actor,
            before,
            after,
        }
    }
    /// The record made before the request is sent.
    pub fn requested(&self) -> EffortLevelChangeRecord {
        self.record(EffortChangeStage::Requested)
    }
    /// The record of how the request settled.
    pub fn settled(&self, outcome: EffortChangeOutcome) -> EffortLevelChangeRecord {
        self.record(match outcome {
            EffortChangeOutcome::Applied => EffortChangeStage::Applied,
            EffortChangeOutcome::Refused => EffortChangeStage::Refused,
            EffortChangeOutcome::Failed => EffortChangeStage::Failed,
        })
    }
    fn record(&self, stage: EffortChangeStage) -> EffortLevelChangeRecord {
        EffortLevelChangeRecord {
            change: self.clone(),
            stage,
        }
    }
}

/// One of the two records of an [`EffortLevelChange`]: the request, or how it
/// settled. Made only from the change, never assembled field by field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffortLevelChangeRecord {
    change: EffortLevelChange,
    stage: EffortChangeStage,
}
impl EffortLevelChangeRecord {
    /// Local session whose attachment the change is for.
    pub fn session_id(&self) -> &SessionId {
        &self.change.session_id
    }
    /// Agent instance and provider generation of that attachment.
    pub fn attachment_generation(&self) -> &str {
        &self.change.attachment_generation
    }
    /// Host-verified caller attribution.
    pub fn actor(&self) -> &ActionContext {
        &self.change.actor
    }
    /// The level in force before the change; `None` is the agent's own default.
    pub fn before(&self) -> Option<&EffortLevel> {
        self.change.before.as_ref()
    }
    /// The level asked for.
    pub fn after(&self) -> &EffortLevel {
        &self.change.after
    }
    /// Where the change stands.
    pub fn stage(&self) -> EffortChangeStage {
        self.stage
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

/// A provider terminal result that arrived after a refusal had already stopped the invocation.
///
/// It is not a second outcome. The local stop stands. This record is only the
/// reply that showed up later, kept so the two facts are not collapsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LateProviderResultRecord {
    session_id: ExecutionSessionId,
    execution_id: ExecutionId,
    result: Result<ExecutionOutcome, AgentError>,
}
impl LateProviderResultRecord {
    /// Record `result` for the invocation `execution_id` inside `session_id`.
    ///
    /// The constructor consumes those identities and the provider's result. It
    /// performs no I/O and does not replace the local stop.
    pub fn new(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        result: Result<ExecutionOutcome, AgentError>,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            result: result.map_err(AgentError::bounded),
        }
    }
    /// Provider session the late reply named.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Invocation the reply belonged to, already stopped locally.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// What the provider reported after the local stop.
    pub fn result(&self) -> &Result<ExecutionOutcome, AgentError> {
        &self.result
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
    /// A caller's live reasoning effort change, asked for and then settled.
    EffortLevelChanged(EffortLevelChangeRecord),
    /// Native model/permission intent and observed settlement.
    ConfigurationControlled(ConfigurationControlRecord),
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
    /// An agent's own question, answered, with that same separation.
    QuestionAnswered(QuestionAnswerRecord),
    /// An agent's question refused by this binding before anybody was asked,
    /// with the same separation of local decision from observed delivery.
    QuestionRefused(QuestionRefusalRecord),
    /// A provider terminal result observed after a refusal had already stopped the invocation.
    LateProviderResult(LateProviderResultRecord),
    /// The hooks choice a Claude or Codex process was started with.
    NativeHooksLaunch(NativeHooksLaunchRecord),
    /// A turn a restart or the host's own shutdown cut off, continued by a
    /// successor turn, or left ended because its chain of resumes is spent.
    TurnResumed(TurnResumptionRecord),
}

/// What resuming did with one owed turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnResumption {
    /// A successor turn was queued to continue it.
    Continued {
        /// The successor's derived identity.
        successor: ExecutionId,
    },
    /// It was cut off [`Resumption::MAX_RESUMES`](crate::domain::agent_execution::executions::Resumption::MAX_RESUMES)
    /// times running and is left ended.
    Exhausted,
    /// The host decided not to resume it ([`SchedulingCause::NotResumed`](crate::domain::agent_execution::executions::SchedulingCause::NotResumed)),
    /// and it is left ended.
    Declined,
}

/// Evidence that an owed turn was resumed: which turn, how it had stopped,
/// whether the provider had been given it, who resumed it, and with what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnResumptionRecord {
    session_id: SessionId,
    execution_id: ExecutionId,
    cause: SchedulingCause,
    dispatched: bool,
    submitted_by: ActionContext,
    resumed_by: ActionContext,
    resumption: TurnResumption,
}
impl TurnResumptionRecord {
    /// `cause` is the owed ending the turn stopped with: a cancellation that
    /// leaves work owed ([`SchedulingCause::leaves_work_owed`]) or a failure
    /// a retry can fix ([`SchedulingCause::stopped_by_failure`]); `dispatched` whether it had
    /// reached the provider; `submitted_by` its original caller and
    /// `resumed_by` the host's verified actor for this resume.
    pub(crate) fn new(
        session_id: SessionId,
        execution_id: ExecutionId,
        cause: SchedulingCause,
        dispatched: bool,
        submitted_by: ActionContext,
        resumed_by: ActionContext,
        resumption: TurnResumption,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            cause,
            dispatched,
            submitted_by,
            resumed_by,
            resumption,
        }
    }
    /// The conversation's session.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    /// The turn that was cut off.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// How it had stopped: suspended by the host's shutdown, interrupted, or ended
    /// by a failure a retry can fix.
    pub fn cause(&self) -> SchedulingCause {
        self.cause
    }
    /// Whether the provider had been given the turn before it stopped.
    pub fn dispatched(&self) -> bool {
        self.dispatched
    }
    /// Who submitted the turn.
    pub fn submitted_by(&self) -> &ActionContext {
        &self.submitted_by
    }
    /// Who resumed it.
    pub fn resumed_by(&self) -> &ActionContext {
        &self.resumed_by
    }
    /// What resuming did with it.
    pub fn resumption(&self) -> &TurnResumption {
        &self.resumption
    }
}

/// Why a Claude or Codex process was started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeHooksLaunchCause {
    /// The first process of a provider session, started by its open. The
    /// open belongs to an attachment, and the attachment record for the same
    /// session names its cause and caller.
    Open,
    /// The first process of a provider opened directly, with no SDK session
    /// and so no attachment. Nothing else records who asked for this open.
    DirectOpen,
    /// The runtime started a replacement after the session's process had
    /// stopped, to run work already admitted. No caller asked for this start.
    Restart,
}

/// The hooks choice recorded when a Claude or Codex process starts.
///
/// `allowed` is the choice that launch used. A later change to the stored
/// setting does not rewrite this record. Other harnesses do not record one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeHooksLaunchRecord {
    session_id: Option<String>,
    launch_id: u64,
    harness: &'static str,
    allowed: bool,
    cause: NativeHooksLaunchCause,
    restored: bool,
}

impl NativeHooksLaunchRecord {
    /// `session_id` is the conversation this launch opened, when the open named one.
    /// `restored` is whether the start resumed saved provider context.
    pub fn new(
        session_id: Option<String>,
        launch_id: u64,
        harness: &'static str,
        allowed: bool,
        cause: NativeHooksLaunchCause,
        restored: bool,
    ) -> Self {
        Self {
            session_id,
            launch_id,
            harness,
            allowed,
            cause,
            restored,
        }
    }

    /// Why this process was started.
    pub fn cause(&self) -> NativeHooksLaunchCause {
        self.cause
    }

    /// Whether this start resumed saved provider context.
    pub fn restored(&self) -> bool {
        self.restored
    }

    /// The conversation, when this launch was for one.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// The id assigned to this process start.
    pub fn launch_id(&self) -> u64 {
        self.launch_id
    }

    /// `claude` or `codex`.
    pub fn harness(&self) -> &'static str {
        self.harness
    }

    /// Whether this launch allowed the harness's own hooks.
    pub fn allowed(&self) -> bool {
        self.allowed
    }
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
    ///
    /// Construction, polling and destruction must not block the runtime thread: caller
    /// deadlines require cooperative asynchronous progress.
    /// The caller may drop this observation after a deadline or cancellation.
    /// Implementations retain ownership of physical I/O already started until it
    /// finishes, and bound their own retained work. Dropping the returned future
    /// does not prove that a write was cancelled or rolled back. The SDK does not
    /// retry an unacknowledged Agent audit record automatically. An adapter must
    /// report success only at its documented durability boundary.
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()>;
}
