//! Stable identity selectors established by application session verification.
use super::{AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId};

#[derive(Clone, Debug, Eq, PartialEq)]
/// Verified identity selectors, not a snapshot of roles or permission decisions.
/// The application constructs this only after checking current credential and
/// membership linkage. Consumers must reauthorize using current state.
pub struct AuthContext {
    principal_id: PrincipalId,
    organization_id: OrganizationId,
    membership_id: MembershipId,
    credential_id: CredentialId,
    audience_id: AudienceId,
}

impl AuthContext {
    /// Assemble already-verified selectors. Each ID must refer to the same
    /// credential/membership relationship checked by the application use case.
    pub(crate) fn new(
        principal_id: PrincipalId,
        organization_id: OrganizationId,
        membership_id: MembershipId,
        credential_id: CredentialId,
        audience_id: AudienceId,
    ) -> Self {
        Self {
            principal_id,
            organization_id,
            membership_id,
            credential_id,
            audience_id,
        }
    }

    /// Principal to which this record is bound.
    pub fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }
    /// Organization that owns this record or resource.
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
    /// Membership used at authentication; resolve its current state for authorization.
    pub fn membership_id(&self) -> &MembershipId {
        &self.membership_id
    }
    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }
    /// Deployment allowed to accept this credential.
    pub fn audience_id(&self) -> &AudienceId {
        &self.audience_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_contains_only_stable_verified_selectors() {
        let context = AuthContext::new(
            PrincipalId::new("principal-1").unwrap(),
            OrganizationId::new("organization-1").unwrap(),
            MembershipId::new("membership-1").unwrap(),
            CredentialId::new("credential-1").unwrap(),
            AudienceId::new("server").unwrap(),
        );
        assert_eq!(context.membership_id().as_str(), "membership-1");
        assert_eq!(context.audience_id().as_str(), "server");
    }
}
