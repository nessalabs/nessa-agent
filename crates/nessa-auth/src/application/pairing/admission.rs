use crate::{
    application::{
        ports::{AccessError, AccessReader, Clock, Decision, PolicyEvaluator},
        session::{AuthenticatedSession, ReadCurrentSession},
    },
    domain::{pairing::ConsentIntent, Action, CredentialId, Resource},
};

/// One current policy decision bound to the exact immutable enrollment intent.
/// Only this use case creates it; the registry checks revision and deadline again.
pub struct PairingAdmission {
    intent: ConsentIntent,
    revision: u64,
    credential: CredentialId,
    deadline_seconds: Option<u64>,
}
impl PairingAdmission {
    /// Exact owner-approved intent covered by the decision.
    pub fn intent(&self) -> &ConsentIntent {
        &self.intent
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn credential(&self) -> &CredentialId {
        &self.credential
    }
    pub(crate) fn deadline_seconds(&self) -> Option<u64> {
        self.deadline_seconds
    }
}
/// Evaluate enrollment management and the proposed read grant with one snapshot.
pub struct AuthorizePairing<'a> {
    /// Current credential and membership authority.
    pub access: &'a dyn AccessReader,
    /// Current authorization policy authority.
    pub policy: &'a dyn PolicyEvaluator,
    /// Absolute time source.
    pub clock: &'a dyn Clock,
}
impl AuthorizePairing<'_> {
    /// Admit this exact intent for its authenticated owner. This grants no device access.
    /// `gateway` is resolved by composition; a caller-selected resource is not authority.
    pub async fn execute(
        &self,
        session: &AuthenticatedSession,
        intent: &ConsentIntent,
        gateway: &Resource,
    ) -> Result<PairingAdmission, AccessError> {
        let snapshot = ReadCurrentSession {
            access: self.access,
            clock: self.clock,
        }
        .execute(session)
        .await?;
        let context = session.context();
        if !intent.matches_owner(
            context.principal_id(),
            context.membership_id(),
            context.audience_id(),
            context.organization_id(),
            gateway,
        ) {
            return Err(AccessError::IdentityMismatch);
        }
        let manage = Action::new("credential.manage").map_err(|_| AccessError::Unavailable)?;
        for (action, resource) in [
            (&manage, gateway),
            (intent.grant().action(), intent.resource()),
        ] {
            if self.policy.evaluate(context, action, resource, &snapshot)? != Decision::Allow {
                return Err(AccessError::InvalidCredential);
            }
        }
        Ok(PairingAdmission {
            intent: intent.clone(),
            revision: snapshot.revision,
            credential: context.credential_id().clone(),
            deadline_seconds: session.expires_at(),
        })
    }
}
