//! Enrollment admission uses current authentication and policy, then registry CAS.
//!
//! ```text
//! authenticated owner -> current snapshot + policy -> admission -> registry
//! enrollment history -------------------------------------------> replay
//! ```
#![deny(missing_docs)]
mod admission;
mod ports;
mod private_state;
mod proof;
mod receiver;
mod stage;
pub use admission::{AuthorizePairing, PairingAdmission};
pub use ports::{
    AttemptReservation, OwnerDecision, PairingStore, PairingStoreError, PairingWorkerFault,
    RuntimeEnd,
};
pub use proof::{ConfirmedClaim, DeviceConnectionProof};
pub use receiver::ReceiverOutcome;

pub(crate) use stage::StageGate;
pub use stage::{
    resolve_terminal_stage, NoReceiverCleanupProof, StageOwnership, StageReceiptLookup,
    StageResolution,
};

pub use private_state::{
    ClientPendingStore, GatewayKeyStore, GatewayPublicationState, PendingEnrollment,
    PrivateKeyMaterial, PrivatePublicationEffect, PrivatePublicationError, PrivatePublicationStep,
    PrivateStateError, PrivateStorageFailure,
};
