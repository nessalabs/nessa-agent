//! Public correlation shared by the PAKE transcript and durable pending state.
use super::{AttemptId, ConsentClass, ConsentIntentId, InvitationId, PairingError};
use crate::domain::pairing::PairingRecord;

/// Public opaque intent correlation, containing no private owner or grant selectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicIntent {
    invitation: InvitationId,
    attempt: AttemptId,
    consent: ConsentIntentId,
    generation: u64,
    expiry_ms: u64,
    class: ConsentClass,
}
impl PublicIntent {
    /// Who the invitation enrolls, shared with the canonical full consent owner
    /// and bound into the PAKE transcript.
    pub fn class(&self) -> ConsentClass {
        self.class
    }

    /// Preserve bounded public metadata; authenticity is established only by PAKE completion.
    pub fn new(
        invitation: InvitationId,
        attempt: AttemptId,
        consent: ConsentIntentId,
        generation: u64,
        expiry_ms: u64,
        class: ConsentClass,
    ) -> Result<Self, PairingError> {
        if generation == 0 || expiry_ms == 0 {
            return Err(PairingError::Invalid);
        }
        Ok(Self {
            invitation,
            attempt,
            consent,
            generation,
            expiry_ms,
            class,
        })
    }
    /// Derive the same public correlation from one canonical registry record.
    /// This discloses no principal/member/org/resource or secret code.
    pub fn from_record(record: &PairingRecord, attempt: AttemptId) -> Result<Self, PairingError> {
        Self::new(
            record.id(),
            attempt,
            record.intent().id(),
            record.intent().generation(),
            record.expires_at_ms(),
            record.intent().class(),
        )
    }
    /// Public invitation locator, never a credential.
    pub fn invitation(&self) -> InvitationId {
        self.invitation
    }
    /// Exact admitted attempt identity.
    pub fn attempt(&self) -> AttemptId {
        self.attempt
    }
    /// Preserve the exact immutable enrollment while selecting another attempt.
    /// This representation operation proves no retry admission or prior outcome.
    pub fn with_attempt(&self, attempt: AttemptId) -> Self {
        Self { attempt, ..*self }
    }
    /// Immutable consent correlation revealed before private selectors.
    pub fn consent(&self) -> ConsentIntentId {
        self.consent
    }
    /// Immutable intent generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Exclusive invitation expiry in Unix milliseconds.
    pub fn expiry_ms(&self) -> u64 {
        self.expiry_ms
    }
}
