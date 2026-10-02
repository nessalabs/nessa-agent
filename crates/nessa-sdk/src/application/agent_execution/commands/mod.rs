//! Durable creation commands, separate from target-owned session content.
//!
//! ```text
//! verified host -> CreationCoordinator -> CreationStorage principal lease
//!                     |-> CreationTarget current access/deletion + initialization
//! principal control -> Accepted -> Attempted -> Ready
//! restart Attempted -> Interrupted (observation only, no provider reopening)
//! ```
//! The coordinator owns effect ordering. The host owns authentication and target
//! deletion; it supplies that verified context rather than an SDK auth port.
#![deny(missing_docs)]

mod creation;
pub use creation::{
    CreationBinding, CreationCoordinator, CreationFailure, CreationFuture,
    CreationInitializationFailure, CreationReceipt, CreationStage, CreationStorage,
    CreationStorageError, CreationStorageLease, CreationTarget, CreationTaskFault,
};

/// Maximum simultaneous principal creation leases on the shared record adapter.
/// A seventeenth owner receives `StorageError::Busy` without a waiting queue.
pub const MAX_CREATION_OWNERS: usize = 16;
