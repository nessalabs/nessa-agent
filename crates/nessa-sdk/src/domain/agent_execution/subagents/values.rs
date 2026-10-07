//! Identities, policy snapshots, and the durable rows a store reloads.
#![deny(missing_docs)]

use super::error::OwnershipError;
use crate::domain::agent_execution::{
    executions::ExecutionId, sessions::SessionId, tools::ToolCallId,
};

/// Live direct children one parent may hold at once.
pub const MAX_DIRECT_CHILDREN: usize = 16;
/// Deepest parent-link distance from a root. The root itself is depth 0.
pub const MAX_DEPTH: u32 = 4;
/// Spawn request bindings retained for conflict detection, including closed ones.
pub const MAX_RETAINED_REQUESTS: usize = 128;
/// Maximum children returned by one relationship read.
pub const MAX_READ_PAGE: usize = 50;
/// Maximum UTF-8 bytes of one bounded ownership label.
pub const MAX_LABEL_BYTES: usize = 256;

fn bounded(value: String, field: &'static str) -> Result<Box<str>, OwnershipError> {
    if value.trim().is_empty() {
        return Err(OwnershipError::EmptyValue(field));
    }
    if value.len() > MAX_LABEL_BYTES {
        return Err(OwnershipError::ValueTooLong {
            field,
            max_bytes: MAX_LABEL_BYTES,
        });
    }
    Ok(value.into_boxed_str())
}

fn portable(value: String, field: &'static str) -> Result<Box<str>, OwnershipError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(OwnershipError::InvalidIdentity(field));
    }
    Ok(value.into_boxed_str())
}

fn approval_policy(
    mode: String,
    offer: String,
    revision: String,
) -> Result<ApprovalPolicy, OwnershipError> {
    Ok(ApprovalPolicy {
        mode: bounded(mode, "approval mode")?,
        offer: bounded(offer, "permission offer")?,
        revision: bounded(revision, "approval revision")?,
    })
}

fn model_choice(provider: String, model: String) -> Result<ModelChoice, OwnershipError> {
    Ok(ModelChoice {
        provider: bounded(provider, "provider")?,
        model: bounded(model, "model")?,
    })
}

fn host_actor(
    principal_id: String,
    surface_id: String,
    request_id: String,
) -> Result<HostActor, OwnershipError> {
    Ok(HostActor {
        principal_id: bounded(principal_id, "principal id")?,
        surface_id: bounded(surface_id, "surface id")?,
        request_id: bounded(request_id, "request id")?,
    })
}

fn agent_lifetime(value: String) -> Result<AgentLifetimeId, OwnershipError> {
    Ok(AgentLifetimeId(portable(value, "agent lifetime id")?))
}

fn spawn_request(value: String) -> Result<SpawnRequestId, OwnershipError> {
    Ok(SpawnRequestId(portable(value, "spawn request id")?))
}

fn close_operation(value: String) -> Result<CloseOperationId, OwnershipError> {
    Ok(CloseOperationId(portable(value, "close operation id")?))
}

fn report_id(value: String) -> Result<ReportId, OwnershipError> {
    Ok(ReportId(portable(value, "report id")?))
}

fn task_receipt(value: String) -> Result<TaskReceiptId, OwnershipError> {
    Ok(TaskReceiptId(portable(value, "task receipt id")?))
}

fn task_digest(value: String) -> Result<TaskDigest, OwnershipError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(OwnershipError::InvalidTaskDigest);
    }
    Ok(TaskDigest(value.into_boxed_str()))
}

/// One opening of a saved conversation for ownership purposes.
/// A later reopen mints another id and does not adopt the previous children.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentLifetimeId(Box<str>);

impl AgentLifetimeId {
    /// Keep a portable key. Blank, oversized, or punctuated input is refused.
    pub fn new(value: impl Into<String>) -> Result<Self, OwnershipError> {
        agent_lifetime(value.into())
    }

    /// Borrow the key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable id of one spawn attempt. Retries reuse it; a changed binding conflicts.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpawnRequestId(Box<str>);

impl SpawnRequestId {
    /// Keep a portable key. Blank, oversized, or punctuated input is refused.
    pub fn new(value: impl Into<String>) -> Result<Self, OwnershipError> {
        spawn_request(value.into())
    }

    /// Borrow the key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identity of one lifetime-close decision. Repeats join this id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CloseOperationId(Box<str>);

impl CloseOperationId {
    /// Keep a portable key. Blank, oversized, or punctuated input is refused.
    pub fn new(value: impl Into<String>) -> Result<Self, OwnershipError> {
        close_operation(value.into())
    }

    /// Borrow the key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Durable id of one child result report into its parent.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReportId(Box<str>);

impl ReportId {
    /// Keep a portable key. Blank, oversized, or punctuated input is refused.
    pub fn new(value: impl Into<String>) -> Result<Self, OwnershipError> {
        report_id(value.into())
    }

    /// Borrow the key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Receipt id of the child's initial task submission.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskReceiptId(Box<str>);

impl TaskReceiptId {
    /// Keep a portable key. Blank, oversized, or punctuated input is refused.
    pub fn new(value: impl Into<String>) -> Result<Self, OwnershipError> {
        task_receipt(value.into())
    }

    /// Borrow the key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// SHA-256 of the delegated task, lowercase hex. The ownership record stores the digest, not the prompt.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TaskDigest(Box<str>);

impl TaskDigest {
    /// Accept exactly 64 lowercase hexadecimal characters.
    pub fn new(value: impl Into<String>) -> Result<Self, OwnershipError> {
        task_digest(value.into())
    }

    /// Borrow the hex digest.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Approval configuration copied onto a child at admission.
/// Mode names are not ordered. Equivalence is exact text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalPolicy {
    mode: Box<str>,
    offer: Box<str>,
    revision: Box<str>,
}

impl ApprovalPolicy {
    /// Retain the mode, permission-offer policy, and configuration revision.
    /// Each must be non-blank and within [`MAX_LABEL_BYTES`].
    pub fn new(
        mode: impl Into<String>,
        offer: impl Into<String>,
        revision: impl Into<String>,
    ) -> Result<Self, OwnershipError> {
        approval_policy(mode.into(), offer.into(), revision.into())
    }

    /// Effective approval mode at child admission.
    pub fn mode(&self) -> &str {
        &self.mode
    }

    /// Permission-offer policy supplied to the child adapter.
    pub fn offer(&self) -> &str {
        &self.offer
    }

    /// Configuration revision that produced this snapshot.
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

/// What the parent mode owner has committed at the moment of selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyRead {
    /// One committed effective policy.
    Committed(ApprovalPolicy),
    /// A mode change is in progress.
    Pending,
    /// The mode owner has no settled result.
    Uncertain,
    /// The parent lifetime is no longer open.
    ParentUnavailable,
}

/// Select the policy a new child inherits.
/// Pending or uncertain input is refused. An unsupported binding is refused.
/// A selected policy is returned unchanged for the caller to store on the binding.
pub fn select_inherited_policy(
    read: PolicyRead,
    child_supports: bool,
) -> Result<ApprovalPolicy, OwnershipError> {
    let policy = match read {
        PolicyRead::Committed(policy) => policy,
        PolicyRead::Pending => return Err(OwnershipError::PolicyPending),
        PolicyRead::Uncertain => return Err(OwnershipError::PolicyUncertain),
        PolicyRead::ParentUnavailable => return Err(OwnershipError::ParentClosing),
    };
    if !child_supports {
        return Err(OwnershipError::UnsupportedPolicy);
    }
    Ok(policy)
}

/// Optional provider and model override resolved by the catalog before admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    provider: Box<str>,
    model: Box<str>,
}

impl ModelChoice {
    /// Retain provider and model identifiers. Neither may be blank.
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, OwnershipError> {
        model_choice(provider.into(), model.into())
    }

    /// Provider identifier.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }
}

/// Verified host actor. Construction checks shape, not access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostActor {
    principal_id: Box<str>,
    surface_id: Box<str>,
    request_id: Box<str>,
}

impl HostActor {
    /// Retain principal, surface, and request identities.
    pub fn new(
        principal_id: impl Into<String>,
        surface_id: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Result<Self, OwnershipError> {
        host_actor(principal_id.into(), surface_id.into(), request_id.into())
    }

    /// Principal the host verified.
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    /// Surface the host verified.
    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }

    /// Logical request id.
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

/// Who initiated a close. A cascaded child keeps the root initiator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Initiator {
    /// A host-verified command.
    Host(HostActor),
    /// Runtime disposal or terminal failure, with no invented person.
    Runtime,
}

/// Why an owned lifetime is ending. Cascaded children copy the root cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifetimeCause {
    /// An authorized caller closed the conversation.
    HostClose,
    /// The lifetime failed in a way that ends ownership.
    TerminalFailure,
    /// The last owning handle was dropped.
    OwnerDisposed,
    /// Deletion requested the tree drain before erasure.
    Deletion,
    /// Approval-mode recovery is retiring this root.
    ModeRecovery,
    /// Gateway shutdown is retiring this root.
    GatewayRetirement,
}

/// Where a spawn was authorized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnOrigin {
    /// An explicit host command.
    Host(HostActor),
    /// A parent execution. Tool id is present when the adapter observed one.
    Execution {
        /// Parent execution that invoked the spawn.
        execution: ExecutionId,
        /// Tool call on that execution, when the adapter supplied one.
        tool: Option<ToolCallId>,
    },
}

/// Immutable comparison key for one admitted spawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnBinding {
    /// Parent lifetime that admitted the child.
    pub parent_lifetime: AgentLifetimeId,
    /// Parent session at admission.
    pub parent_session: SessionId,
    /// Spawn request id.
    pub request_id: SpawnRequestId,
    /// Digest of the delegated task.
    pub task_digest: TaskDigest,
    /// Policy snapshot copied at admission.
    pub policy: ApprovalPolicy,
    /// Optional catalog-resolved model override.
    pub model: Option<ModelChoice>,
    /// Verified origin.
    pub origin: SpawnOrigin,
}

/// How far a spawn's own admission chart has moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnownMilestone {
    /// Binding reserved, provider not prepared.
    Reserved,
    /// Child prepared, not yet attached.
    Prepared,
    /// Attached to the parent, task not admitted.
    Attached,
    /// Initial task submission acknowledged.
    TaskAdmitted {
        /// Submission receipt.
        receipt: TaskReceiptId,
    },
}

/// Spawn chart position. Task admission and drained endings are different terminals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnProgress {
    /// Reserved under the parent.
    Reserved,
    /// Factory prepared the child.
    Prepared,
    /// Ownership attachment acknowledged.
    Attached,
    /// Initial task admitted once.
    TaskAdmitted {
        /// Submission receipt.
        receipt: TaskReceiptId,
    },
    /// A write or provider attempt was interrupted. Lookup may confirm a milestone.
    Unconfirmed {
        /// Last milestone known before the interruption.
        known: KnownMilestone,
    },
    /// This reservation must not be dispatched. The parent is closing, or the
    /// child lifetime is already closing or closed.
    Draining {
        /// Last milestone known when the drain took it.
        known: KnownMilestone,
    },
    /// Startup failed and still holds cleanup ownership.
    StartupFailed {
        /// Last milestone known when startup failed.
        known: KnownMilestone,
    },
    /// Drain or failed startup settled. This is not task admission.
    Ended {
        /// Last milestone known when it ended.
        known: KnownMilestone,
    },
}

impl SpawnProgress {
    /// Last milestone this position still names.
    pub fn known(&self) -> KnownMilestone {
        match self {
            Self::Reserved => KnownMilestone::Reserved,
            Self::Prepared => KnownMilestone::Prepared,
            Self::Attached => KnownMilestone::Attached,
            Self::TaskAdmitted { receipt } => KnownMilestone::TaskAdmitted {
                receipt: receipt.clone(),
            },
            Self::Unconfirmed { known }
            | Self::Draining { known }
            | Self::StartupFailed { known }
            | Self::Ended { known } => known.clone(),
        }
    }
}

/// Open, closing, or closed. Physical cleanup and audit are separate facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifetimeState {
    /// Children may be admitted.
    Open,
    /// Admission is sealed. Cleanup may still be running.
    Closing,
    /// Parent and descendant resources and required evidence are settled.
    Closed,
}

/// Physical cleanup of one target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalFact {
    /// Not yet confirmed.
    Pending,
    /// The owner confirmed release.
    Released,
    /// The owner reported a typed failure. Another attempt may still release it.
    Failed,
}

/// Audit acknowledgement for one target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceFact {
    /// Not yet acknowledged.
    Pending,
    /// The audit port acknowledged the record.
    Acknowledged,
    /// The audit port rejected or lost the record.
    Failed,
}

/// Meaning named in an ownership evidence record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnershipMeaning {
    /// No prior relationship row.
    Absent,
    /// Lifetime is open.
    Open,
    /// Spawn reserved.
    Reserved,
    /// Spawn prepared.
    Prepared,
    /// Spawn attached.
    Attached,
    /// Initial task admitted.
    TaskAdmitted,
    /// Progress uncertain.
    Unconfirmed,
    /// Spawn is draining.
    Draining,
    /// Spawn ended without task admission.
    Ended,
    /// Startup failed and still owns cleanup.
    StartupFailed,
    /// Lifetime is closing.
    Closing,
    /// Lifetime is closed.
    Closed,
    /// Report admitted to the parent.
    Submitted,
    /// Report suppressed because the parent lifetime was not open.
    Suppressed,
}

/// One consequential ownership transition. The application audits it before success.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub struct OwnershipEvidence {
    /// Typed close observation, absence or aggregate completion authority.
    pub close_detail: Option<CloseEvidenceDetail>,
    /// Parent lifetime this record is about. For a root open, this is the root.
    pub parent_lifetime: AgentLifetimeId,
    /// Child lifetime, when the transition has one.
    pub child_lifetime: Option<AgentLifetimeId>,
    /// Close operation, when the transition is part of a close.
    pub close_operation: Option<CloseOperationId>,
    /// Meaning before the transition.
    pub before: OwnershipMeaning,
    /// Meaning after the transition.
    pub after: OwnershipMeaning,
    /// Close cause when this transition seals or joins a close.
    pub cause: Option<LifetimeCause>,
    /// Known initiator. Runtime when no person acted.
    pub initiator: Initiator,
}

/// A durable lifetime row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifetimeRow {
    /// Lifetime identity.
    pub lifetime_id: AgentLifetimeId,
    /// Session this opening belongs to.
    pub session_id: SessionId,
    /// Open, closing, or closed.
    pub state: LifetimeState,
    /// Close operation, once sealing has started.
    pub close_operation: Option<CloseOperationId>,
    /// First close cause.
    pub cause: Option<LifetimeCause>,
    /// First close initiator.
    pub initiator: Option<Initiator>,
    /// Ancestor whose close cascaded here. Empty for a direct close.
    pub cascaded_from: Option<AgentLifetimeId>,
}

/// A durable spawn binding and its progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRow {
    /// Child lifetime.
    pub child_lifetime: AgentLifetimeId,
    /// Child session.
    pub child_session: SessionId,
    /// Immutable binding.
    pub binding: SpawnBinding,
    /// Chart position.
    pub progress: SpawnProgress,
}

/// Physical and audit facts for one target of a close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettlementRow {
    /// Actual resource observations or validated live absence, including exact audit debt.
    pub proof: SettlementProof,
    /// Lifetime whose close this fact belongs to.
    pub close_lifetime: AgentLifetimeId,
    /// Target inside that close.
    pub target: AgentLifetimeId,
    /// Physical cleanup fact.
    pub physical: PhysicalFact,
    /// Audit fact.
    pub evidence: EvidenceFact,
}

/// A durable result-report row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportRow {
    /// Report identity.
    pub report_id: ReportId,
    /// Child whose result this is.
    pub child_lifetime: AgentLifetimeId,
    /// Parent that would receive it.
    pub parent_lifetime: AgentLifetimeId,
    /// Delivery chart position.
    pub state: DeliveryState,
}

/// Result delivery position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryState {
    /// Retained, not yet offered to the parent.
    Retained,
    /// Admitted while the parent lifetime was open.
    Submitted,
    /// Not admitted because the parent lifetime was closing or closed.
    Suppressed,
    /// The admission write was interrupted.
    Unconfirmed,
}

/// Rows the store reloads. [`super::OwnershipGraph::restore`] decides whether they may dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct OwnershipSnapshot {
    /// Explicit first-close aggregate completion facts, one per direct close root.
    pub close_completions: Vec<CloseCompletionRow>,
    /// Lifetime rows.
    pub lifetimes: Vec<LifetimeRow>,
    /// Spawn rows.
    pub spawns: Vec<SpawnRow>,
    /// Settlement rows.
    pub settlements: Vec<SettlementRow>,
    /// Result report rows.
    pub reports: Vec<ReportRow>,
}

/// Actual absence observed by the live transaction, never inferred during restoration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbsenceProof {
    /// A root that has never transferred resources or a gate.
    NeverTransferredRoot,
    /// Actual Ready preparation rejection promised no outstanding cleanup owner.
    PreparationRejectedWithoutOwner(SpawnRequestId),
    /// This admitted invocation failed publication before its factory handoff.
    AdmissionFailedBeforeFactory(SpawnRequestId),
}

/// Consequential close fact delivered to the mandatory ownership audit port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseEvidenceDetail {
    /// The first actual observation of this physical outcome.
    ResourceObservation {
        /// Actual physical outcome, not an inference from audit delivery.
        physical: PhysicalFact,
        /// Provider evidence scoped to this physical outcome.
        provider_evidence: EvidenceFact,
    },
    /// Actual absence has no synthetic provider acknowledgement.
    Absence(AbsenceProof),
    /// Aggregate readiness decision, separately acknowledged before Closed.
    Completion,
}

/// Immutable first observation and its independent acknowledgements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceObservationAudit {
    /// Exact first observation delivered on every coordinator retry.
    pub(super) record: OwnershipEvidence,
    /// Actual coordinator acknowledgement; Acknowledged is absorbing.
    pub(super) acknowledgement: EvidenceFact,
    /// Actual same-outcome provider witness, including later correlated acknowledgement.
    pub(super) provider_acknowledged: bool,
}

/// Actual absence and the exact mandatory coordinator audit obligation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbsenceAudit {
    /// Live transaction's validated absence authority.
    pub(super) proof: AbsenceProof,
    /// Exact immutable absence observation.
    pub(super) record: OwnershipEvidence,
    /// Actual ownership audit acknowledgement.
    pub(super) acknowledgement: EvidenceFact,
}

/// Bounded close evidence. Resource slots are Pending, Failed, Released in that order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettlementProof {
    /// At most three immutable first observations and scoped provider witnesses.
    Resource(Box<[Option<ResourceObservationAudit>; 3]>),
    /// Actual correlated absence, with no provider field.
    Absence(AbsenceAudit),
}

/// Root-only aggregate completion authority retained by the ownership graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseCompletionRow {
    /// Direct first-close root whose owned targets were ready.
    pub(super) close_lifetime: AgentLifetimeId,
    /// Immutable explicitly typed aggregate decision.
    pub(super) record: OwnershipEvidence,
    /// Actual completion audit acknowledgement; only Ack changes lifetimes to Closed.
    pub(super) acknowledgement: EvidenceFact,
}

impl ResourceObservationAudit {
    /// Exact immutable first-observation record.
    pub fn record(&self) -> &OwnershipEvidence {
        &self.record
    }
    /// Actual coordinator acknowledgement of this record.
    pub fn acknowledgement(&self) -> EvidenceFact {
        self.acknowledgement
    }
    /// Whether an actual provider witness for this outcome was observed.
    pub fn provider_acknowledged(&self) -> bool {
        self.provider_acknowledged
    }
    pub(crate) fn from_parts(
        record: OwnershipEvidence,
        acknowledgement: EvidenceFact,
        provider_acknowledged: bool,
    ) -> Self {
        Self {
            record,
            acknowledgement,
            provider_acknowledged,
        }
    }
}
impl AbsenceAudit {
    /// Actual live proof, validated with the enclosing graph on restoration.
    pub fn proof(&self) -> &AbsenceProof {
        &self.proof
    }
    /// Exact immutable absence observation.
    pub fn record(&self) -> &OwnershipEvidence {
        &self.record
    }
    /// Actual coordinator acknowledgement of the absence record.
    pub fn acknowledgement(&self) -> EvidenceFact {
        self.acknowledgement
    }
    pub(crate) fn from_parts(
        proof: AbsenceProof,
        record: OwnershipEvidence,
        acknowledgement: EvidenceFact,
    ) -> Self {
        Self {
            proof,
            record,
            acknowledgement,
        }
    }
}
impl CloseCompletionRow {
    /// First direct operation owner, which may itself be a child lifetime.
    pub fn close_lifetime(&self) -> &AgentLifetimeId {
        &self.close_lifetime
    }
    /// Exact immutable explicitly typed aggregate decision.
    pub fn record(&self) -> &OwnershipEvidence {
        &self.record
    }
    /// Actual mandatory audit acknowledgement of this decision.
    pub fn acknowledgement(&self) -> EvidenceFact {
        self.acknowledgement
    }
    pub(crate) fn from_parts(
        close_lifetime: AgentLifetimeId,
        record: OwnershipEvidence,
        acknowledgement: EvidenceFact,
    ) -> Self {
        Self {
            close_lifetime,
            record,
            acknowledgement,
        }
    }
}
