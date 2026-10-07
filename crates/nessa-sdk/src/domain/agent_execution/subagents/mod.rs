//! Parent ownership for ordinary child agents.
//!
//! ```text
//! caller -> OwnershipGraph -> lifetimes + spawn bindings + close facts
//! ```
//!
//! The arrow is a method call. The graph decides which relationship changes are
//! legal. It does not read a clock, a database, or a provider. Application code
//! audits the evidence these methods return and performs effects outside the decision.
//! Targeted private-root discard preserves neighboring live/history rows. A
//! correlated UnboundRootSettlement consumes the actual absence-audit result;
//! the application supplies proof that no physical owner was transferred.
mod error;
mod graph;
mod values;

pub use error::OwnershipError;
pub use graph::{
    ChildPage, ChildView, CloseAdmission, CloseCompletion, Dispatch, OwnershipGraph, SpawnAdmission,
    UnboundRootSettlement,
};
pub use values::{
    select_inherited_policy, AbsenceAudit, AbsenceProof, AgentLifetimeId, CloseCompletionRow, CloseEvidenceDetail, ResourceObservationAudit, SettlementProof, ApprovalPolicy, CloseOperationId, DeliveryState,
    EvidenceFact, HostActor, Initiator, KnownMilestone, LifetimeCause, LifetimeRow, LifetimeState,
    ModelChoice, OwnershipEvidence, OwnershipMeaning, OwnershipSnapshot, PhysicalFact, PolicyRead,
    ReportId, ReportRow, SettlementRow, SpawnBinding, SpawnOrigin, SpawnProgress, SpawnRequestId,
    SpawnRow, TaskDigest, TaskReceiptId, MAX_DEPTH, MAX_DIRECT_CHILDREN, MAX_LABEL_BYTES,
    MAX_READ_PAGE, MAX_RETAINED_REQUESTS,
};
