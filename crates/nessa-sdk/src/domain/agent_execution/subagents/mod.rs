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
//! `graph::settlement` owns exact bounded observation debts and derives first-close
//! ownership from cascade ancestry. Explicit root-only Completion acknowledges
//! readiness after independent child closes, separately from observations.
//! The application supplies actual live absence proof and the durable Closing
//! writer barrier; restoration validates retained proof without inferring it.
mod error;
mod graph;
mod values;

pub use error::OwnershipError;
pub use graph::{
    ChildPage, ChildView, CloseAdmission, CloseCompletion, Dispatch, OwnershipGraph,
    SpawnAdmission, UnboundRootSettlement,
};
pub use values::{
    select_inherited_policy, AbsenceAudit, AbsenceProof, AgentLifetimeId, ApprovalPolicy,
    CloseCompletionRow, CloseEvidenceDetail, CloseOperationId, DeliveryState, EvidenceFact,
    HostActor, Initiator, KnownMilestone, LifetimeCause, LifetimeRow, LifetimeState, ModelChoice,
    OwnershipEvidence, OwnershipMeaning, OwnershipSnapshot, PhysicalFact, PolicyRead, ReportId,
    ReportRow, ResourceObservationAudit, SettlementProof, SettlementRow, SpawnBinding, SpawnOrigin,
    SpawnProgress, SpawnRequestId, SpawnRow, TaskDigest, TaskReceiptId, MAX_DEPTH,
    MAX_DIRECT_CHILDREN, MAX_LABEL_BYTES, MAX_READ_PAGE, MAX_RETAINED_REQUESTS,
};
