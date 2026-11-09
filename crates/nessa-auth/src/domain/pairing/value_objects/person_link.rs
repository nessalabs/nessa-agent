//! A gateway attribution is an identity fact, separate from every grant.
use super::{InvitationId, PairingError, PairingInitiator};
use crate::domain::{
    pairing::{PairingPhase, PairingRecord, TerminalCause},
    Credential, CredentialId, CredentialTransition, OrganizationId, PrincipalId, TransitionCause,
};

/// Original cause of an attribution insert or removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonLinkCause {
    /// The owner chose this association on the invitation.
    SamePersonChoice,
    /// The associated credential was revoked.
    CredentialRevoked,
    /// The associated credential reached its exclusive expiry.
    CredentialExpired,
    /// The owner ended the enrollment.
    EnrollmentRevoked,
}
/// Original initiator of the effect, retained when another process retries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersonLinkInitiator {
    /// A verified principal caused the invitation or its revocation.
    Principal(PrincipalId),
    /// The registry's offline owner revoked the credential.
    LocalOperator,
    /// Exclusive credential expiry caused removal.
    Expiry,
}
/// Desired attribution effect derived from one enrollment's current evidence.
/// This has no grant capability or admission decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonLinkChange {
    organization: OrganizationId,
    gateway: PrincipalId,
    person: PrincipalId,
    credential: CredentialId,
    invitation: InvitationId,
    cause: PersonLinkCause,
    initiator: PersonLinkInitiator,
}
impl PersonLinkChange {
    /// Validate related credential, phase and revocation evidence together.
    /// A pending invitation cannot yield an association.
    pub fn from_record(
        record: &PairingRecord,
        credential: &Credential,
        revocation: Option<&CredentialTransition>,
        now_seconds: u64,
    ) -> Result<Option<Self>, PairingError> {
        if !record.intent().same_person() {
            return Ok(None);
        }
        if !record.intent().class().is_peer()
            || record.credential() != Some(credential.id())
            || record.credential_principal().as_ref() != Some(credential.principal_id())
            || record.intent().resource().organization_id() != credential.organization_id()
            || record.intent().audience() != credential.audience_id()
            || !credential.grants().iter().eq(record.intent().grants())
        {
            return Err(PairingError::Conflict);
        }
        if !matches!(
            record.phase(),
            PairingPhase::Active | PairingPhase::Terminal
        ) {
            return Ok(None);
        }
        let actor = |actor: &PairingInitiator| match actor {
            PairingInitiator::Principal(id) => Ok(PersonLinkInitiator::Principal(id.clone())),
            PairingInitiator::LocalOperator => Ok(PersonLinkInitiator::LocalOperator),
            PairingInitiator::System | PairingInitiator::Device(_) => Err(PairingError::WrongActor),
        };
        let revoked = credential
            .revoked_at()
            .map(|_| checked_revocation(credential, revocation))
            .transpose()?;
        if revocation.is_some() && revoked.is_none() {
            return Err(PairingError::Conflict);
        }
        if let Some((terminal, initiator)) = record.terminal() {
            if !matches!(
                terminal,
                TerminalCause::Cancelled | TerminalCause::CredentialRevoked
            ) {
                return Err(PairingError::Conflict);
            }
            let revoked = revoked.ok_or(PairingError::Conflict)?;
            if &PairingInitiator::from_credential(revoked.initiator()) != initiator {
                return Err(PairingError::WrongActor);
            }
        }
        // The fixed expiry and retained revocation time preserve which boundary
        // ended this credential first, including a retry after the other event.
        let expired_first = credential.expires_at().is_some_and(|expiry| {
            credential
                .revoked_at()
                .map_or(now_seconds >= expiry, |revoked| expiry <= revoked)
        });
        let (cause, initiator) = if expired_first {
            (
                PersonLinkCause::CredentialExpired,
                PersonLinkInitiator::Expiry,
            )
        } else if let Some((TerminalCause::Cancelled, initiator)) = record.terminal() {
            (PersonLinkCause::EnrollmentRevoked, actor(initiator)?)
        } else if let Some(revoked) = revoked {
            (
                PersonLinkCause::CredentialRevoked,
                actor(&PairingInitiator::from_credential(revoked.initiator()))?,
            )
        } else if record.phase() == PairingPhase::Active {
            if record.receiver_binding().is_none() {
                return Err(PairingError::Conflict);
            }
            (
                PersonLinkCause::SamePersonChoice,
                PersonLinkInitiator::Principal(record.intent().owner().clone()),
            )
        } else {
            return Ok(None);
        };
        Ok(Some(Self {
            organization: credential.organization_id().clone(),
            gateway: credential.principal_id().clone(),
            person: record.intent().owner().clone(),
            credential: credential.id().clone(),
            invitation: record.id(),
            cause,
            initiator,
        }))
    }
    /// Organization of both principals.
    pub fn organization(&self) -> &OrganizationId {
        &self.organization
    }
    /// The gateway principal whose credential still names it.
    pub fn gateway(&self) -> &PrincipalId {
        &self.gateway
    }
    /// The person this gateway acts for, independently of authority.
    pub fn person(&self) -> &PrincipalId {
        &self.person
    }
    /// Exact enrollment credential; later credentials do not inherit this fact.
    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }
    /// Original invitation identity.
    pub fn invitation(&self) -> InvitationId {
        self.invitation
    }
    /// The original transition cause.
    pub fn cause(&self) -> PersonLinkCause {
        self.cause
    }
    /// The original initiator.
    pub fn initiator(&self) -> &PersonLinkInitiator {
        &self.initiator
    }
    /// Whether the effect inserts an association rather than removing it.
    pub fn links(&self) -> bool {
        self.cause == PersonLinkCause::SamePersonChoice
    }
}
fn checked_revocation<'a>(
    credential: &Credential,
    transition: Option<&'a CredentialTransition>,
) -> Result<&'a CredentialTransition, PairingError> {
    let transition = transition.ok_or(PairingError::Conflict)?;
    if transition.credential_id() != credential.id()
        || !matches!(transition.cause(), TransitionCause::Revoked(_))
        || transition.after().issued_at != credential.issued_at()
        || transition.after().expires_at != credential.expires_at()
        || transition.after().revoked_at != credential.revoked_at()
        || credential.revoked_at().is_none()
    {
        return Err(PairingError::Conflict);
    }
    Ok(transition)
}
