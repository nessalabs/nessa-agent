//! Structural validation for an app binding supplied by the credential owner.
//!
//! An issuance journal entry alone is not app approval. The approved binding
//! must be read separately from the same committed credential state. This
//! value does not check Cedar policy, verify a secret, or admit a session.
use super::{
    origin::CredentialOrigin,
    ports::{AccessError, AccessSnapshot, CredentialEvidence, PortFuture},
};
use crate::domain::{
    AudienceId, CredentialId, ExtensionAppId, Initiator, IssuanceCause, OrganizationId, PrincipalId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Exact operations that owner approval can name for this slice.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppCapability {
    List,
    Read,
    Create,
    Subscribe,
    Stop,
}

/// Nonempty, bounded set of typed operations. No dynamic operation names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppCapabilities(BTreeSet<AppCapability>);

impl AppCapabilities {
    pub fn new(first: AppCapability, others: &[AppCapability]) -> Self {
        Self(
            std::iter::once(first)
                .chain(others.iter().copied())
                .collect(),
        )
    }

    pub(crate) fn restore(operations: &[AppCapability]) -> Result<Self, AccessError> {
        let [first, rest @ ..] = operations else {
            return Err(AccessError::Unavailable);
        };
        let capabilities = Self::new(*first, rest);
        if capabilities.iter().eq(operations.iter().copied()) {
            Ok(capabilities)
        } else {
            Err(AccessError::Unavailable)
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = AppCapability> + '_ {
        self.0.iter().copied()
    }
}

/// One approved owner command. This is not a deserializable Product request.
pub struct IssueAppCredential {
    pub request_id: String,
    pub credential_id: CredentialId,
    pub app: ExtensionAppId,
    pub capabilities: AppCapabilities,
    pub expires_at: u64,
}

/// Secret-free facts retained by the credential owner for one approved issue.
/// This input is for an authority adapter, not for Product request decoding.
pub struct ApprovedAppIssue {
    pub app: ExtensionAppId,
    pub capabilities: AppCapabilities,
    pub organization: OrganizationId,
    pub audience: AudienceId,
    pub subject: PrincipalId,
    pub approving_owner: PrincipalId,
    pub origin: CredentialOrigin,
}

/// One app namespace and the exact capabilities bound by that approval.
/// Obtained only by matching an authority-provided approval to its issuance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppCredentialBinding {
    app: ExtensionAppId,
    capabilities: AppCapabilities,
}

impl AppCredentialBinding {
    /// Resolve the approval for this app credential in an existing access read.
    /// Missing app approval has no default: this seam is only for app lookups.
    pub fn lookup(
        snapshot: &AccessSnapshot,
        approval: Option<&ApprovedAppIssue>,
    ) -> Result<Self, AccessError> {
        let approval = approval.ok_or(AccessError::Unavailable)?;
        let credential = &snapshot.credential;
        if snapshot.origin.cause() != IssuanceCause::AppIssue
            || approval.origin != snapshot.origin
            || !snapshot.origin.matches(credential, snapshot.revision)
            || snapshot.origin.correlation().is_none()
            || snapshot.origin.transition().initiator()
                != &Initiator::Principal(approval.approving_owner.clone())
            || credential.organization_id() != &approval.organization
            || credential.audience_id() != &approval.audience
            || credential.principal_id() != &approval.subject
        {
            return Err(AccessError::IdentityMismatch);
        }
        Ok(Self {
            app: approval.app.clone(),
            capabilities: approval.capabilities.clone(),
        })
    }

    pub fn app(&self) -> &ExtensionAppId {
        &self.app
    }

    pub fn capabilities(&self) -> &AppCapabilities {
        &self.capabilities
    }
}

/// Current access and approved app scope read from one committed revision.
pub struct AppCredentialSnapshot {
    pub(crate) access: AccessSnapshot,
    pub(crate) binding: AppCredentialBinding,
}

/// App proof and revalidation, owned by the existing credential registry.
pub trait AppCredentials: Send + Sync {
    fn verify_app<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, AppCredentialSnapshot>;

    fn read_app<'a>(
        &'a self,
        credential: &'a CredentialId,
    ) -> PortFuture<'a, AppCredentialSnapshot>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::dto::CredentialTransitionDto,
        domain::{
            Credential, CredentialTransition, Membership, MembershipId, MembershipRole,
            MembershipStatus, TransitionCause,
        },
    };

    fn fixture(id: &str, receipt: Option<&str>, app: &str) -> (AccessSnapshot, ApprovedAppIssue) {
        let subject = PrincipalId::new(id).unwrap();
        let organization = OrganizationId::new("organization").unwrap();
        let audience = AudienceId::new("gateway").unwrap();
        let approving_owner = PrincipalId::new("owner").unwrap();
        let credential = Credential::new(
            CredentialId::new(id).unwrap(),
            subject.clone(),
            organization.clone(),
            audience.clone(),
            10,
            20,
            Vec::new(),
        )
        .unwrap();
        let transition = CredentialTransition::new(
            credential.id().clone(),
            None,
            credential.lifecycle(),
            TransitionCause::Issued(IssuanceCause::AppIssue),
            Initiator::Principal(approving_owner.clone()),
            credential.issued_at(),
        )
        .unwrap();
        let origin = CredentialOrigin::restore(
            CredentialTransitionDto::record(&transition, 1, 1, receipt.map(str::to_owned)),
            &credential,
        )
        .unwrap();
        let membership = Membership::new(
            MembershipId::new(id).unwrap(),
            subject.clone(),
            organization.clone(),
            MembershipRole::Member,
            MembershipStatus::Active,
        );
        let snapshot = AccessSnapshot {
            credential,
            membership,
            origin: origin.clone(),
            revocation: None,
            revision: 1,
        };
        let approval = ApprovedAppIssue {
            app: ExtensionAppId::new(app).unwrap(),
            capabilities: AppCapabilities::new(AppCapability::Read, &[AppCapability::List]),
            organization,
            audience,
            subject,
            approving_owner,
            origin,
        };
        (snapshot, approval)
    }

    #[test]
    fn lookup_exposes_the_exact_approved_extension_and_operations() {
        let (snapshot, approval) = fixture("credential-a", Some("approve-a"), "notebook");
        let binding = AppCredentialBinding::lookup(&snapshot, Some(&approval)).unwrap();
        assert_eq!(binding.app().as_str(), "notebook");
        assert_eq!(
            binding.capabilities().iter().collect::<Vec<_>>(),
            [AppCapability::List, AppCapability::Read]
        );
    }

    #[test]
    fn lookup_needs_an_approval_and_a_correlated_owner_issuance() {
        let (snapshot, _) = fixture("credential-a", Some("approve-a"), "notebook");
        assert!(matches!(
            AppCredentialBinding::lookup(&snapshot, None),
            Err(AccessError::Unavailable)
        ));
        let (snapshot, approval) = fixture("credential-a", None, "notebook");
        assert!(matches!(
            AppCredentialBinding::lookup(&snapshot, Some(&approval)),
            Err(AccessError::IdentityMismatch)
        ));
    }

    #[test]
    fn lookup_refuses_a_different_credential_or_issue_receipt() {
        let (snapshot, _) = fixture("credential-a", Some("approve-a"), "notebook");
        let (_, other_app) = fixture("credential-b", Some("approve-b"), "calendar");
        let (_, other_receipt) = fixture("credential-a", Some("approve-b"), "notebook");
        for approval in [&other_app, &other_receipt] {
            assert!(matches!(
                AppCredentialBinding::lookup(&snapshot, Some(approval)),
                Err(AccessError::IdentityMismatch)
            ));
        }
    }

    #[test]
    fn lookup_refuses_owner_binding_mismatch_or_future_issue() {
        let (mut snapshot, mut approval) = fixture("credential-a", Some("approve-a"), "notebook");
        approval.approving_owner = PrincipalId::new("another-owner").unwrap();
        assert!(matches!(
            AppCredentialBinding::lookup(&snapshot, Some(&approval)),
            Err(AccessError::IdentityMismatch)
        ));
        approval.approving_owner = PrincipalId::new("owner").unwrap();
        approval.audience = AudienceId::new("another-gateway").unwrap();
        assert!(matches!(
            AppCredentialBinding::lookup(&snapshot, Some(&approval)),
            Err(AccessError::IdentityMismatch)
        ));
        approval.audience = AudienceId::new("gateway").unwrap();
        snapshot.revision = 0;
        assert!(matches!(
            AppCredentialBinding::lookup(&snapshot, Some(&approval)),
            Err(AccessError::IdentityMismatch)
        ));
    }
}
