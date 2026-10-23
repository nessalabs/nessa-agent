//! Original issuance provenance, restored from the authority's retained journal.
//! This value validates recorded facts; it never reconstructs missing history.
use super::{dto::CredentialTransitionDto, ports::AccessError};
use crate::domain::{Credential, CredentialTransition, IssuanceCause, TransitionCause};

/// One original issuance and its actual journal identity and command correlation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialOrigin {
    transition: CredentialTransition,
    sequence: u64,
    revision: u64,
    correlation: Option<Box<str>>,
}

impl CredentialOrigin {
    /// Decode an actual retained issuance record, bound to current metadata.
    /// The current credential may be revoked; its original issuance is not.
    pub fn restore(
        mut record: CredentialTransitionDto,
        current: &Credential,
    ) -> Result<Self, AccessError> {
        let sequence = record.sequence;
        let revision = record.revision;
        let correlation = record.correlation.take();
        if sequence == 0
            || revision == 0
            || correlation
                .as_ref()
                .is_some_and(|id| id.trim().is_empty() || id.len() > 200)
        {
            return Err(AccessError::Unavailable);
        }
        let transition =
            CredentialTransition::try_from(record).map_err(|_| AccessError::Unavailable)?;
        if !matches!(transition.cause(), TransitionCause::Issued(_)) {
            return Err(AccessError::Unavailable);
        }
        if matches!(
            transition.cause(),
            TransitionCause::Issued(IssuanceCause::WebhookIssue)
        ) && (correlation.is_none()
            || !matches!(
                transition.initiator(),
                crate::domain::Initiator::Principal(_)
            ))
        {
            return Err(AccessError::Unavailable);
        }
        let origin = Self {
            transition,
            sequence,
            revision,
            correlation: correlation.map(String::into_boxed_str),
        };
        if !origin.matches(current, u64::MAX) {
            return Err(AccessError::IdentityMismatch);
        }
        Ok(origin)
    }

    /// Recheck cross-record binding when consuming a supplied access snapshot.
    pub fn matches(&self, current: &Credential, revision: u64) -> bool {
        self.revision <= revision
            && self.transition.credential_id() == current.id()
            && self.transition.after().issued_at == current.issued_at()
            && self.transition.after().expires_at == current.expires_at()
    }

    /// Original domain transition, including its original actor and time.
    pub fn transition(&self) -> &CredentialTransition {
        &self.transition
    }
    /// Actual command retry identity, absent for commands without one.
    pub fn correlation(&self) -> Option<&str> {
        self.correlation.as_deref()
    }
    /// Stable journal sequence, retained through credential revocation.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// Original commit revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Original issuing use case. No purpose is inferred from grants.
    pub fn cause(&self) -> IssuanceCause {
        match self.transition.cause() {
            TransitionCause::Issued(cause) => *cause,
            TransitionCause::Revoked(_) => unreachable!("origin restoration accepts issuance only"),
        }
    }

    /// An older admin issuance does not establish webhook provenance. Retain
    /// its actual actor and command, and require explicit revoke/reissue recovery.
    /// This classifies recorded provenance; it does not grant permission.
    pub fn requires_webhook_recovery(&self) -> bool {
        match self.cause() {
            IssuanceCause::AdminIssue => true,
            IssuanceCause::Bootstrap
            | IssuanceCause::WebhookIssue
            | IssuanceCause::DevicePairing
            | IssuanceCause::SurfaceProvision
            | IssuanceCause::OwnerRecovery => false,
        }
    }
}
