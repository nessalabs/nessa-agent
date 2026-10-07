//! Ports the ownership coordinator uses for effects outside a transition.
#![deny(missing_docs)]

use std::{
    fmt::{Debug, Formatter, Result as FmtResult},
    sync::Arc,
};

use async_trait::async_trait;

use crate::application::agent_execution::agents::OwnedLifetime;

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
    /// Restoration found contradictory ownership history; transfer is refused.
    RefusedHistory,
    /// The identity exists privately but its ownership admission is not acknowledged.
    UnpublishedLifetime,
    /// The lifetime already completed closure.
    Closed,
    /// An admitted child transaction still owns its factory transfer slot.
    FactoryInFlight,
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
    pub resources: Arc<dyn ChildResources>,
}
impl Debug for BindResourcesFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
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
#[derive(Clone)]
pub struct PrepareRequest {
    /// Accepted-transaction participation for installation on the child Agent.
    /// This is the coordinator's actual child gate, sharing its admission scope
    /// and seal. Preparation failure revokes new attachment authority immediately;
    /// revocation does not prove physical release. Return any unfinished cleanup.
    pub owned_lifetime: Arc<dyn OwnedLifetime>,
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
impl Debug for PrepareRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("PrepareRequest")
            .field("owned_lifetime", &"<owned lifetime>")
            .field("parent", &self.parent)
            .field("child", &self.child)
            .field("session", &self.session)
            .field("policy", &self.policy)
            .field("model", &self.model)
            .field("task", &self.task)
            .field("request", &self.request)
            .field("origin", &self.origin)
            .finish()
    }
}

/// A prepared child that has not been dispatched.
#[derive(Clone)]
pub struct PreparedChild {
    /// Cleanup owner, including when later dispatch is refused.
    pub resources: Arc<dyn ChildResources>,
    /// Initial task submission.
    pub submit: Arc<dyn InitialSubmit>,
}

/// Factory failure. Any cleanup owner is retained by the coordinator.
/// `Rejected` with no cleanup promises that no attachment or cleanup remains.
/// A supplied owner carries unfinished cleanup; revoking the gate proves no release.
pub struct PrepareFailure {
    /// Whether preparation was refused or uncertain.
    pub failure: PortFailure,
    /// Resources to close when preparation held them.
    pub cleanup: Option<Arc<dyn ChildResources>>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::{agents::AgentError, permissions::ActionContext};
    use crate::domain::agent_execution::subagents::HostActor;
    use std::sync::{atomic::AtomicBool, Arc, Mutex};
    struct NoInspection;
    #[async_trait]
    impl OwnedLifetime for NoInspection {
        fn admission_scope(&self) -> Arc<Mutex<()>> {
            panic!("Debug must not inspect a gate")
        }
        fn seal(&self) -> Arc<AtomicBool> {
            panic!("Debug must not inspect a gate")
        }
        fn seal_for_disposal(&self) {
            panic!("Debug must not operate a gate")
        }
        async fn seal_for_host(&self, _: &ActionContext) -> Result<(), AgentError> {
            panic!("unexpected")
        }
        async fn note_attachment(&self, _: bool, _: bool) {
            panic!("unexpected")
        }
        async fn join_descendants(&self) -> Result<(), AgentError> {
            panic!("unexpected")
        }
    }
    #[test]
    fn row_34_prepare_request_debug_does_not_call_gate_contract() {
        let request = PrepareRequest {
            owned_lifetime: Arc::new(NoInspection),
            parent: AgentLifetimeId::new("parent").unwrap(),
            child: AgentLifetimeId::new("child").unwrap(),
            session: SessionId::new("session").unwrap(),
            policy: ApprovalPolicy::new("read-only", "ask", "revision").unwrap(),
            model: None,
            task: "task".into(),
            request: SpawnRequestId::new("request").unwrap(),
            origin: SpawnOrigin::Host(HostActor::new("person", "desktop", "request").unwrap()),
        };
        let cloned = request.clone();
        assert!(Arc::ptr_eq(&request.owned_lifetime, &cloned.owned_lifetime));
        assert!(format!("{request:?}").contains("<owned lifetime>"));
    }
}
