//! One-use device invitation decisions and their immutable causal evidence.
//!
//! ```text
//! validated values -> PairingRecord -> PairingTransition -> replay
//! ```
//! Arrows denote pure construction/decisions; proof verification, authorization,
//! clocks and persistence belong to the consuming application and adapters.
#![deny(missing_docs)]
mod aggregates;
mod value_objects;

pub use aggregates::{
    validate_pairing_collection, AttemptFailure, AttemptOutcome, PairingEvent, PairingPhase,
    PairingRecord, PairingTransition, TerminalCause, MAX_LIVE_PAIRINGS,
};
pub use value_objects::{
    is_peer_principal, peer_principal, AttemptId, ConsentClass, ConsentIntent, ConsentIntentId,
    DeviceKey, DisclosedConsent, InvitationId, PairingError, PairingInitiator, PairingPolicy,
    PublicIntent, PEER_PRINCIPAL_PREFIX,
};

pub(crate) use aggregates::Event;
pub(crate) use value_objects::MANUAL_CODE_BYTES;
