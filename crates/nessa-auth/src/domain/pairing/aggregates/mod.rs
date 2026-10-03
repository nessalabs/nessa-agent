//! One record orders attempts, key consent, staging and terminal causes.
//! Its transition correlates Active cancellation with the canonical credential revocation.
mod invitation;
pub use invitation::{
    AttemptFailure, AttemptOutcome, PairingEvent, PairingPhase, PairingRecord, PairingTransition,
    TerminalCause,
};

pub(crate) use invitation::Event;

mod collection;
pub use collection::{validate_pairing_collection, MAX_LIVE_PAIRINGS};
