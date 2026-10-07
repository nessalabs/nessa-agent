//! Ports the ownership coordinator uses for effects outside a transition.
#![deny(missing_docs)]

use async_trait::async_trait;

use crate::domain::agent_execution::sessions::SessionId;
use crate::domain::agent_execution::subagents::{
    AgentLifetimeId, ApprovalPolicy, EvidenceFact, Initiator, LifetimeCause, ModelChoice,
    OwnershipEvidence, OwnershipSnapshot, PhysicalFact, SpawnOrigin, SpawnRequestId, TaskReceiptId,
};

/// A port rejected the attempt, or the acknowledgement was lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortFailure {
    /// The effect was refused and did not happen.
    Rejected,
    /// The effect may have happened. Do not repeat it blindly.
    Uncertain,
}

/// Physical and audit facts a child cleanup owner reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceReport {
    /// Whether the owner confirmed release.
    pub physical: PhysicalFact,
    /// Whether required evidence was acknowledged.
    pub evidence: EvidenceFact,
}

/// Cleanup ownership for one prepared or running child.
#[async_trait]
pub trait ChildResources: Send + Sync {
    /// Close this child. The cause and initiator are the root close, not a new person.
    async fn close(&self, cause: &LifetimeCause, initiator: &Initiator) -> ResourceReport;
}

/// Why a lifetime did not accept a cleanup owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindResourcesRefusal {
    /// The identity is not present in the graph.
    UnknownLifetime,
    /// The lifetime already completed closure.
    Closed,
    /// A cleanup owner is already retained; it cannot be replaced.
    AlreadyBound,
    /// Physical absence or release was confirmed before this binding.
    Released,
}

/// Failed resource transfer. The caller retains responsibility for this owner.
/// Inspect `reason`, recover `resources`, and close or retain it explicitly.
#[must_use = "a refused cleanup owner remains the caller's responsibility"]
pub struct BindResourcesFailure {
    /// Typed refusal; no physical resource transfer occurred.
    pub reason: BindResourcesRefusal,
    /// The exact rejected owner, returned without closing or replacing it.
    pub resources: std::sync::Arc<dyn ChildResources>,
}
impl std::fmt::Debug for BindResourcesFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BindResourcesFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// The child's initial task submission. Retries use the spawn request id.
#[async_trait]
pub trait InitialSubmit: Send + Sync {
    /// Admit the task once. Uncertain means the caller must not submit again.
    async fn submit(
        &self,
        task: &str,
        request: &SpawnRequestId,
    ) -> Result<TaskReceiptId, PortFailure>;
}

/// What the factory receives after the binding is acknowledged.
#[derive(Clone, Debug)]
pub struct PrepareRequest {
    /// Parent lifetime.
    pub parent: AgentLifetimeId,
    /// Child lifetime minted for this binding.
    pub child: AgentLifetimeId,
    /// Child session minted for this binding.
    pub session: SessionId,
    /// Policy snapshot to apply.
    pub policy: ApprovalPolicy,
    /// Optional catalog model.
    pub model: Option<ModelChoice>,
    /// Delegated task text. The ownership record stores only its digest.
    pub task: String,
    /// Spawn request id.
    pub request: SpawnRequestId,
    /// Verified origin.
    pub origin: SpawnOrigin,
}

/// A prepared child that has not been dispatched.
#[derive(Clone)]
pub struct PreparedChild {
    /// Cleanup owner, including when later dispatch is refused.
    pub resources: std::sync::Arc<dyn ChildResources>,
    /// Initial task submission.
    pub submit: std::sync::Arc<dyn InitialSubmit>,
}

/// Factory failure. Any cleanup owner is retained by the coordinator.
pub struct PrepareFailure {
    /// Whether preparation was refused or uncertain.
    pub failure: PortFailure,
    /// Resources to close when preparation held them.
    pub cleanup: Option<std::sync::Arc<dyn ChildResources>>,
}

/// Prepares an ordinary child. It must not dispatch work.
#[async_trait]
pub trait ChildFactory: Send + Sync {
    /// Prepare the child described by `request`.
    async fn prepare(&self, request: PrepareRequest) -> Result<PreparedChild, PrepareFailure>;
}

/// Mandatory audit of ownership evidence.
#[async_trait]
pub trait OwnershipAudit: Send + Sync {
    /// Record one transition. Failure is visible to the caller.
    async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure>;
}

/// Acknowledged ownership rows.
#[async_trait]
pub trait OwnershipStore: Send + Sync {
    /// Replace the retained snapshot after a transition.
    async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure>;
    /// Load the retained snapshot. An empty store returns an empty snapshot.
    async fn read(&self) -> Result<OwnershipSnapshot, PortFailure>;
}

/// The existing live-conversation capacity owner.
pub trait LiveRoom: Send + Sync {
    /// Reserve one live slot. False means the spawn must be refused.
    fn try_reserve(&self) -> bool;
    /// Return one slot after a reserved child is released or the reserve is unused.
    fn release(&self);
}
