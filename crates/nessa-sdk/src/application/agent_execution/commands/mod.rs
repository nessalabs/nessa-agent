//! Durable principal commands, separate from target-owned session content.
//!
//! ```text
//! verified host -> CreationCoordinator / MutationCoordinator
//!                     |-> request control lease (one stream per principal request)
//!                     |-> host current access/deletion + one original effect
//! creation: Accepted -> Attempted -> Ready
//! submit/stop: Accepted -> Attempted -> Settled
//!              Accepted -> Settled(AlreadyFinal) when stop sends nothing
//!              Attempted -> Settled(NoEffect) when the effect refuses before dispatch
//!              Attempted -> Settled(RefusedAfterSave) when a saved steer is refused before injection
//!              Attempted -> Settled(UnavailableAfterSave) when a saved steer cannot be checked before injection
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
