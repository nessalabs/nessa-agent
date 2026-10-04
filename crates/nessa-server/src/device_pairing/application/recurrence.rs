//! Whether an enrollment failure will recur if the same step is tried again.
//!
//! The one classifier for it: approval's `activationStopped` (retryable or
//! permanent) and startup's restart policy for an unfinished cleanup both
//! derive from these functions, so the owner and the relaunch decision cannot
//! disagree (design rows A11, S8, S16).
use super::receivers::ReceiverError;
use nessa_auth::application::{
    pairing::{PairingStoreError, PrivateStateError},
    ports::AccessError,
};

/// A registry failure that refuses the same way on the same records.
pub fn store_error_recurs(error: PairingStoreError) -> bool {
    match error {
        PairingStoreError::GatewayKeyHistoryExists
        | PairingStoreError::NotFound
        | PairingStoreError::Domain(_)
        | PairingStoreError::PrivateState(
            PrivateStateError::Corrupt | PrivateStateError::Conflict,
        ) => true,
        PairingStoreError::StageOccupied
        | PairingStoreError::WorkerFault(_)
        | PairingStoreError::StaleRevision
        | PairingStoreError::Unavailable
        | PairingStoreError::PrivateState(
            PrivateStateError::AuditUnavailable(_)
            | PrivateStateError::Publication(_)
            | PrivateStateError::AuditPublication { .. }
            | PrivateStateError::Locked
            | PrivateStateError::Unavailable
            | PrivateStateError::Uncertain,
        ) => false,
    }
}

/// A receiver-authority failure that refuses the same way on the same journal.
pub fn receiver_error_recurs(error: ReceiverError) -> bool {
    match error {
        ReceiverError::Conflict
        | ReceiverError::Missing
        | ReceiverError::Exhausted
        | ReceiverError::NotPaired => true,
        ReceiverError::Unavailable => false,
    }
}

/// An owner authorization failure that re-authenticating cannot clear. An
/// expired session or inactive membership can: the owner signs in again.
pub fn access_error_recurs(error: AccessError) -> bool {
    match error {
        AccessError::Denied
        | AccessError::InvalidCredential
        | AccessError::CredentialRevoked
        | AccessError::IdentityMismatch
        | AccessError::Unsupported => true,
        AccessError::Unavailable
        | AccessError::StaleRevision
        | AccessError::CredentialExpired
        | AccessError::InactiveMembership => false,
    }
}
