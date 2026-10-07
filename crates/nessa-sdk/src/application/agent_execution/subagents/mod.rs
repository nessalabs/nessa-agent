//! Coordinates parent and child agent lifetimes.
//!
//! ```text
//! caller -> OwnershipCoordinator -> OwnershipGraph
//!                                -> OwnershipStore / OwnershipAudit / ChildFactory
//! graph + publication eligibility -> projected snapshot -> write fence -> OwnershipStore
//! root transaction -> delivery ticket -> caller (or owned reconciliation)
//! ```
//!
//! Arrows are calls. The graph decides the transition. Storage, audit, and the
//! child factory run after that decision returns, and their results are applied
//! as later correlated transitions. The write fence drops an older snapshot copy
//! after a newer copy has been acknowledged. `publication` owns audit eligibility;
//! `root` owns admission and unclaimed-result reconciliation. The domain graph
//! owns lifecycle and correlated settlement. The coordinator does not run a model loop.
#![deny(missing_docs)]

mod coordinator;
mod failure;
mod memory;
mod ports;
mod publication;
mod root;

pub use coordinator::{
    CloseCommand, OwnershipCoordinator, OwnershipDependencies, SpawnCommand, SpawnReceipt,
};
pub use failure::OwnershipFailure;
pub use memory::{LiveCapacity, MemoryOwnershipStore};
pub use ports::{
    BindResourcesFailure, BindResourcesRefusal, ChildFactory, ChildResources, InitialSubmit,
    LiveRoom, OwnershipAudit, OwnershipStore, PortFailure, PrepareFailure, PrepareRequest,
    PreparedChild, ResourceReport,
};
