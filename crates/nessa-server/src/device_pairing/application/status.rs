//! Canonical attempt meaning passed from the application owner to representation.
use nessa_auth::domain::pairing::{AttemptOutcome, PairingRecord, TerminalCause};

/// Trusted application projection of an earlier device attempt.
///
/// Construction is not key proof or current authority. The query owner
/// must obtain this through the canonical store and actual device possession.
/// The wire adapter validates claimed record correlation before serialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevicePairingStatus {
    /// The original attempt may still claim; no terminal cause is inferred.
    Pending,
    /// A failed/superseded attempt without private grant disclosure.
    Unclaimed {
        /// Original failed or superseded outcome, not Pending/Claimed.
        outcome: AttemptOutcome,
        /// Original invitation terminal cause, if it has ended.
        terminal: Option<TerminalCause>,
    },
    /// Immutable record belonging to this claimed attempt.
    /// Active is historical completion; it does not authorize a product request.
    Claimed(Box<PairingRecord>),
}
