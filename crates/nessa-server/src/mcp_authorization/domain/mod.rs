//! Per-server authorization transitions. A call returns the next value and
//! the effects the owner must perform; nothing here waits on the network.
//!
//! ```text
//! Command ──▶ ServerAuth::step ──▶ Decision { auth, effects, refusal }
//! ```
//!
//! The chart is ADR 392. Ready's token availability and refresh activity
//! change together in one step. A stale generation cannot publish.

mod machine;

pub use machine::{
    AcceptedDiscovery, Admission, Attempt, Command, Decision, Deletion, Effect, Obligation, Phase,
    Publication, RefreshActivity, Refusal, RemoteObservation, RevokeCause, ServerAuth, Settlement,
    TokenAvailability, CONSENT_DEADLINE_MS,
};
