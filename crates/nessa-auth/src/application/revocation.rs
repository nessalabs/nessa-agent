//! Actual immutable revocation provenance from the existing auth journal.
use super::{
    credential_journal::JournalIdentity, dto::CredentialTransitionDto, ports::AccessError,
};
use crate::domain::{Credential, CredentialTransition, TransitionCause};

/// One canonical revocation, including explicit/superseded cause and the actual
/// principal or offline initiator. This is evidence, not permission to act.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRevocation {
    transition: CredentialTransition,
    journal: JournalIdentity,
}
impl CredentialRevocation {
    /// Decode an actual retained revocation and bind it to current metadata.
    /// Missing records must remain absent; callers cannot infer an actor.
    pub fn restore(
        mut record: CredentialTransitionDto,
        current: &Credential,
    ) -> Result<Self, AccessError> {
        let journal = JournalIdentity::take(&mut record)?;
        let transition =
            CredentialTransition::try_from(record).map_err(|_| AccessError::Unavailable)?;
        if !matches!(transition.cause(), TransitionCause::Revoked(_)) {
            return Err(AccessError::Unavailable);
        }
        let fact = Self {
            transition,
            journal,
        };
        if !fact.matches(current, u64::MAX) {
            return Err(AccessError::IdentityMismatch);
        }
        Ok(fact)
    }
    /// Check the current credential and consuming snapshot revision together.
    pub fn matches(&self, current: &Credential, revision: u64) -> bool {
        self.journal.revision() <= revision
            && self.transition.credential_id() == current.id()
            && self.transition.after().issued_at == current.issued_at()
            && self.transition.after().expires_at == current.expires_at()
            && self.transition.after().revoked_at == current.revoked_at()
    }
    /// Original transition, retaining its original actor, cause and command time.
    pub fn transition(&self) -> &CredentialTransition {
        &self.transition
    }
    /// Actual command correlation, absent for actions without a command key.
    pub fn correlation(&self) -> Option<&str> {
        self.journal.correlation()
    }
    /// Stable original journal sequence.
    pub fn sequence(&self) -> u64 {
        self.journal.sequence()
    }
    /// Original commit revision.
    pub fn revision(&self) -> u64 {
        self.journal.revision()
    }
}
