//! Coordinates parent and child agent lifetimes.
//!
//! ```text
//! caller -> OwnershipCoordinator -> OwnershipGraph
//!                                -> OwnershipStore / OwnershipAudit / ChildFactory
//! ```
//!
//! Arrows are calls. The graph decides the transition. Storage, audit, and the
//! child factory run after that decision returns, and their results are applied
//! as later correlated transitions. The coordinator does not run a model loop.
#![deny(missing_docs)]

mod coordinator;
mod failure;
mod memory;
mod ports;

pub use coordinator::{
    CloseCommand, OwnershipCoordinator, OwnershipDependencies, SpawnCommand, SpawnReceipt,
};
pub use failure::OwnershipFailure;
pub use memory::{LiveCapacity, MemoryOwnershipStore};
pub use ports::{
    ChildFactory, ChildResources, InitialSubmit, LiveRoom, OwnershipAudit, OwnershipStore,
    PortFailure, PrepareFailure, PrepareRequest, PreparedChild, ResourceReport,
};
