//! Durable principal commands, separate from target-owned session content.
//!
//! ```text
//! verified host -> CreationCoordinator / MutationCoordinator
//!                     |-> request owner + shared principal receipt writer
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

/// Resource class of a request-bound command handle; this grants no permission.
/// Writable variants claim the same principal/request key across command families.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandAccess {
    /// Observe one request without creating a missing stream or admitting effects.
    ReadOnly,
    /// Own creation progress and its original initialization using ordinary capacity.
    Creation,
    /// Own submit progress and its original dispatch using ordinary capacity.
    Submit,
    /// Own exact-turn Stop progress using capacity reserved for control commands.
    Stop,
}

/// Maximum simultaneous ordinary command owners on one record-storage adapter.
/// Creation and submit share these sixteen slots; retained operations keep their
/// slot until completion. Admission beyond the bound returns `StorageError::Busy`.
pub const MAX_COMMAND_OWNERS: usize = 16;

/// Maximum simultaneous Stop owners on one record-storage adapter.
/// These four slots are independent of ordinary command capacity. A duplicate
/// principal/request cannot acquire a second owner through another resource class.
pub const MAX_STOP_OWNERS: usize = 4;

/// Maximum simultaneous observational command handles on one record adapter.
/// These four slots admit no effect and create no missing command stream.
pub const MAX_COMMAND_READS: usize = 4;
