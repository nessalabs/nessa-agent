//! Durable principal commands, separate from target-owned session content.
//!
//! ```text
//! verified host -> CreationCoordinator / MutationCoordinator
//!                     |-> principal control lease (one request namespace)
//!                     |-> host current access/deletion + one original effect
//! creation: Accepted -> Attempted -> Ready
//! submit/stop: Accepted -> Attempted -> Settled
//!              Accepted -> Settled(AlreadyFinal) when stop sends nothing
//! restart Attempted -> Interrupted (observation only, no repeated effect)
//! ```
//! The coordinator owns effect ordering. The host owns authentication and target
//! deletion; it supplies that verified context rather than an SDK auth port.
#![deny(missing_docs)]

mod creation;
mod mutation;
pub use creation::{
    CreationBinding, CreationCoordinator, CreationFailure, CreationFuture,
    CreationInitializationFailure, CreationReceipt, CreationStage, CreationStorage,
    CreationStorageError, CreationStorageLease, CreationTarget, CreationTaskFault,
};
pub use mutation::{
    MutationBinding, MutationCoordinator, MutationFailure, MutationOperation, MutationOutcome,
    MutationPlan, MutationReceipt, MutationStage, MutationTarget,
};

/// Maximum simultaneous principal creation leases on the shared record adapter.
/// A seventeenth owner receives `StorageError::Busy` without a waiting queue.
pub const MAX_CREATION_OWNERS: usize = 16;
