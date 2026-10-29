//! Actual immutable revocation provenance from the existing auth journal.
use super::{
    credential_journal::JournalIdentity, dto::CredentialTransitionDto, ports::AccessError,
};
use crate::domain::{Credential, CredentialTransition, RevocationCause, TransitionCause};

/// Validated shape of one secret-free retained revocation record. Decoding does
/// not establish journal origin or current credential binding, and grants no
/// authority. Live consumers use the actual held AccessSnapshot instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationRecord {
    transition: CredentialTransition,
    journal: JournalIdentity,
}
impl RevocationRecord {
    /// Decode with the existing journal identity and domain transition owners.
    /// Historical consumers must correlate the record with their retained target.
    pub fn decode(mut record: CredentialTransitionDto) -> Result<Self, AccessError> {
        let journal = JournalIdentity::take(&mut record)?;
        let transition =
            CredentialTransition::try_from(record).map_err(|_| AccessError::Unavailable)?;
        if !matches!(transition.cause(), TransitionCause::Revoked(_)) {
            return Err(AccessError::Unavailable);
        }
        Ok(Self {
            transition,
            journal,
        })
    }
    /// Original domain transition, preserving actor, cause and command time.
    pub fn transition(&self) -> &CredentialTransition {
        &self.transition
    }
    /// Revocation cause guaranteed by this record's canonical decoder.
    pub fn cause(&self) -> &RevocationCause {
        match self.transition.cause() {
            TransitionCause::Revoked(cause) => cause,
            TransitionCause::Issued(_) => unreachable!("record decoding accepts revocation only"),
        }
    }
    /// Original command correlation, absent where no command key existed.
    pub fn correlation(&self) -> Option<&str> {
        self.journal.correlation()
    }
    /// Original journal sequence.
    pub fn sequence(&self) -> u64 {
        self.journal.sequence()
    }
    /// Original commit revision.
    pub fn revision(&self) -> u64 {
        self.journal.revision()
    }
    /// Secret-free persisted representation of these unchanged original facts.
    pub fn to_dto(&self) -> CredentialTransitionDto {
        CredentialTransitionDto::record(
            &self.transition,
            self.sequence(),
            self.revision(),
            self.correlation().map(str::to_owned),
        )
    }
}

/// One canonical revocation bound to current metadata, including explicit or
/// superseded cause and the actual principal or offline initiator. This is
/// evidence, not permission to act.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRevocation {
    record: RevocationRecord,
}
impl CredentialRevocation {
    /// Decode an actual retained revocation and bind it to current metadata.
    /// Missing records must remain absent; callers cannot infer an actor.
    pub fn restore(
        record: CredentialTransitionDto,
        current: &Credential,
    ) -> Result<Self, AccessError> {
        let fact = Self {
            record: RevocationRecord::decode(record)?,
        };
        if !fact.matches(current, u64::MAX) {
            return Err(AccessError::IdentityMismatch);
        }
        Ok(fact)
    }
    /// Check the current credential and consuming snapshot revision together.
    pub fn matches(&self, current: &Credential, revision: u64) -> bool {
        let transition = self.transition();
        self.revision() <= revision
            && transition.credential_id() == current.id()
            && transition.after().issued_at == current.issued_at()
            && transition.after().expires_at == current.expires_at()
            && transition.after().revoked_at == current.revoked_at()
    }
    /// Borrow the validated retained record without conferring current authority.
    pub fn retained_record(&self) -> &RevocationRecord {
        &self.record
    }
    /// Original transition, retaining its original actor, cause and command time.
    pub fn transition(&self) -> &CredentialTransition {
        self.record.transition()
    }
    /// Actual command correlation, absent for actions without a command key.
    pub fn correlation(&self) -> Option<&str> {
        self.record.correlation()
    }
    /// Stable original journal sequence.
    pub fn sequence(&self) -> u64 {
        self.record.sequence()
    }
    /// Original commit revision.
    pub fn revision(&self) -> u64 {
        self.record.revision()
    }
}
