//! Domain records with private fields and explicit construction rules.
//! IDs describe ownership and linkage; callers resolve whether those records exist.
//! All timestamps are Unix seconds. No model reads time or performs persistence.
use super::{
    Action, AudienceId, CredentialId, CredentialLifecycle, CredentialTransition, DomainError,
    Initiator, IssuanceCause, MembershipId, OrganizationId, PrincipalId, ResourceId,
    RevocationCause, Supersession, TransitionCause,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Kind of actor. This classification conveys no permission or role.
pub enum PrincipalKind {
    Human,
    Agent,
    Integration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Stable identity of a human, agent, or integration; membership is separate.
pub struct Principal {
    id: PrincipalId,
    kind: PrincipalKind,
}

impl Principal {
    /// Create an actor from a stable `id` and descriptive `kind`; no membership is created.
    pub fn new(id: PrincipalId, kind: PrincipalKind) -> Self {
        Self { id, kind }
    }

    /// Stable identifier of this record.
    pub fn id(&self) -> &PrincipalId {
        &self.id
    }

    /// Actor classification; not an authorization role.
    pub fn kind(&self) -> PrincipalKind {
        self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Resource ownership boundary. A local personal organization needs no hosted account.
pub struct Organization {
    id: OrganizationId,
}

impl Organization {
    /// Create an ownership container from its stable `id`; no online account is required.
    pub fn new(id: OrganizationId) -> Self {
        Self { id }
    }

    /// Stable identifier of this record.
    pub fn id(&self) -> &OrganizationId {
        &self.id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Role within one organization. Policy and credential limits still apply to admins.
pub enum MembershipRole {
    Admin,
    Member,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Whether this membership may currently participate in authentication.
pub enum MembershipStatus {
    Active,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// A principal’s role and current membership state in one organization.
pub struct Membership {
    id: MembershipId,
    principal_id: PrincipalId,
    organization_id: OrganizationId,
    role: MembershipRole,
    status: MembershipStatus,
}

impl Membership {
    /// Create a membership linking `principal_id` to `organization_id`.
    /// `id` identifies the membership record; `role` and `status` come from the
    /// configured membership authority. This constructor does not authorize a role change.
    pub fn new(
        id: MembershipId,
        principal_id: PrincipalId,
        organization_id: OrganizationId,
        role: MembershipRole,
        status: MembershipStatus,
    ) -> Self {
        Self {
            id,
            principal_id,
            organization_id,
            role,
            status,
        }
    }

    /// Stable identifier of this record.
    pub fn id(&self) -> &MembershipId {
        &self.id
    }
    /// Principal to which this record is bound.
    pub fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }
    /// Organization that owns this record or resource.
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
    /// Current organization role; callers must not cache it as permanent permission.
    pub fn role(&self) -> MembershipRole {
        self.role
    }
    /// Current membership state supplied by its authority.
    pub fn status(&self) -> MembershipStatus {
        self.status
    }
    /// Whether the membership is active; this alone does not grant any action.
    pub fn is_active(&self) -> bool {
        self.status == MembershipStatus::Active
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Exact resource identity together with its authoritative owning organization.
pub struct Resource {
    organization_id: OrganizationId,
    id: ResourceId,
}

impl Resource {
    /// Identify `id` inside its owning `organization_id`.
    /// The caller must resolve ownership from trusted state and use IDs that are
    /// unique across resource kinds within the organization.
    pub fn new(organization_id: OrganizationId, id: ResourceId) -> Self {
        Self {
            organization_id,
            id,
        }
    }

    /// Organization that owns this record or resource.
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
    /// Stable identifier of this record.
    pub fn id(&self) -> &ResourceId {
        &self.id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Permission limit for one exact action and resource. No wildcard matching is implied.
pub struct Grant {
    action: Action,
    resource: Resource,
}

impl Grant {
    /// Limit access to the supplied `action` on exactly `resource`; policy must still allow it.
    pub fn new(action: Action, resource: Resource) -> Self {
        Self { action, resource }
    }
    /// Exact application-owned action name.
    pub fn action(&self) -> &Action {
        &self.action
    }
    /// Exact resource covered by this grant.
    pub fn resource(&self) -> &Resource {
        &self.resource
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Secret-free credential metadata bound to one principal, organization, and audience.
pub struct Credential {
    id: CredentialId,
    principal_id: PrincipalId,
    organization_id: OrganizationId,
    audience_id: AudienceId,
    issued_at: u64,
    expires_at: Option<u64>,
    revoked_at: Option<u64>,
    grants: Vec<Grant>,
}

impl Credential {
    /// Build metadata for a newly issued, unrevoked credential.
    ///
    /// `id` identifies the binding, `principal_id` the actor, `organization_id` the
    /// tenant, and `audience_id` the deployment allowed to accept the proof.
    /// `issued_at` is inclusive and `expires_at` exclusive, both in Unix seconds.
    /// `grants` may be empty; every resource must belong to `organization_id`.
    ///
    /// Returns an error if expiry is not after issuance or a grant crosses tenants.
    /// This validates metadata only; it neither mints nor verifies a secret.
    pub fn new(
        id: CredentialId,
        principal_id: PrincipalId,
        organization_id: OrganizationId,
        audience_id: AudienceId,
        issued_at: u64,
        expires_at: impl Into<Option<u64>>,
        grants: Vec<Grant>,
    ) -> Result<Self, DomainError> {
        let expires_at = expires_at.into();
        if let Some(expires_at) = expires_at.filter(|expiry| *expiry <= issued_at) {
            return Err(DomainError::InvalidCredentialLifetime {
                issued_at,
                expires_at,
            });
        }
        if grants
            .iter()
            .any(|grant| grant.resource().organization_id() != &organization_id)
        {
            return Err(DomainError::GrantOrganizationMismatch);
        }
        Ok(Self {
            id,
            principal_id,
            organization_id,
            audience_id,
            issued_at,
            expires_at,
            revoked_at: None,
            grants,
        })
    }

    /// Stable identifier of this record.
    pub fn id(&self) -> &CredentialId {
        &self.id
    }
    /// Principal to which this record is bound.
    pub fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }
    /// Organization that owns this record or resource.
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
    /// Deployment allowed to accept this credential.
    pub fn audience_id(&self) -> &AudienceId {
        &self.audience_id
    }
    /// Inclusive start of validity, in Unix seconds.
    pub fn issued_at(&self) -> u64 {
        self.issued_at
    }
    /// Exclusive end of validity, in Unix seconds.
    pub fn expires_at(&self) -> Option<u64> {
        self.expires_at
    }
    /// Recorded revocation time in Unix seconds, or None if never revoked.
    pub fn revoked_at(&self) -> Option<u64> {
        self.revoked_at
    }
    /// Credential restrictions; an empty list grants no product actions.
    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }

    /// Check `now` (Unix seconds) against issuance, exclusive expiry, and revocation.
    /// Any recorded revocation denies use, even when `now` precedes that record.
    /// This is a lifetime check, not proof verification or permission evaluation.
    pub fn is_valid_at(&self, now: u64) -> bool {
        self.revoked_at.is_none()
            && self.issued_at <= now
            && self.expires_at.is_none_or(|expiry| now < expiry)
    }

    /// The lifecycle fields a transition can change.
    pub fn lifecycle(&self) -> CredentialLifecycle {
        CredentialLifecycle {
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            revoked_at: self.revoked_at,
        }
    }

    /// Evidence that this credential was created by `cause` on behalf of
    /// `initiator`. Only a credential that has never been revoked can be issued.
    pub fn issued(
        &self,
        cause: IssuanceCause,
        initiator: Initiator,
    ) -> Result<CredentialTransition, DomainError> {
        CredentialTransition::new(
            self.id.clone(),
            None,
            self.lifecycle(),
            TransitionCause::Issued(cause),
            initiator,
            self.issued_at,
        )
    }

    /// Restore a stored revocation time under the same rule as a live one.
    /// Storage calls this while rebuilding a credential; it yields no evidence
    /// because the change already happened.
    pub fn restore_revoked_at(&mut self, revoked_at: u64) -> Result<(), DomainError> {
        self.record_revocation(revoked_at).map(|_| ())
    }

    /// Deliberately revoke at `revoked_at` (Unix seconds), including after expiry,
    /// on behalf of `initiator`. Repeated calls succeed without changing the
    /// original record and return no new evidence. A first revocation cannot
    /// precede issuance: the time is the caller's request, so the caller is told.
    /// The caller must persist the change and its evidence together before
    /// acknowledging it or publishing an invalidation.
    pub fn revoke(
        &mut self,
        revoked_at: u64,
        initiator: Initiator,
    ) -> Result<Option<CredentialTransition>, DomainError> {
        let before = self.lifecycle();
        if !self.record_revocation(revoked_at)? {
            return Ok(None);
        }
        Ok(Some(
            CredentialTransition::new(
                self.id.clone(),
                Some(before),
                self.lifecycle(),
                TransitionCause::Revoked(RevocationCause::Explicit),
                initiator,
                revoked_at,
            )
            .expect("an accepted revocation satisfies the transition rule"),
        ))
    }

    /// Retire this credential automatically because `by` replaces it, at the
    /// later of `at` and this credential's own issuance. An automatic
    /// supersession has no caller to correct an odd timestamp, so it is clamped
    /// forward rather than rejected. Already-revoked credentials are unchanged
    /// and yield no evidence.
    pub fn supersede(
        &mut self,
        at: u64,
        by: CredentialId,
        kind: Supersession,
        initiator: Initiator,
    ) -> Option<CredentialTransition> {
        let before = self.lifecycle();
        let revoked_at = at.max(self.issued_at);
        if !self
            .record_revocation(revoked_at)
            .expect("a clamped supersession never precedes issuance")
        {
            return None;
        }
        Some(
            CredentialTransition::new(
                self.id.clone(),
                Some(before),
                self.lifecycle(),
                TransitionCause::Revoked(RevocationCause::Superseded { by, kind }),
                initiator,
                at,
            )
            .expect("an accepted supersession satisfies the transition rule"),
        )
    }

    /// Keep the first recorded revocation; refuse one before issuance. Returns
    /// whether the record changed.
    fn record_revocation(&mut self, revoked_at: u64) -> Result<bool, DomainError> {
        if self.revoked_at.is_some() {
            return Ok(false);
        }
        if revoked_at < self.issued_at {
            return Err(DomainError::RevokedBeforeIssued {
                issued_at: self.issued_at,
                revoked_at,
            });
        }
        self.revoked_at = Some(revoked_at);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(grant_organization: &str) -> Result<Credential, DomainError> {
        Credential::new(
            CredentialId::new("credential-1").unwrap(),
            PrincipalId::new("principal-1").unwrap(),
            OrganizationId::new("organization-1").unwrap(),
            AudienceId::new("server").unwrap(),
            100,
            200,
            vec![Grant::new(
                Action::new("conversation.read").unwrap(),
                Resource::new(
                    OrganizationId::new(grant_organization).unwrap(),
                    ResourceId::new("conversation-1").unwrap(),
                ),
            )],
        )
    }

    #[test]
    fn absent_expiry_still_enforces_issuance_and_revocation() {
        let mut credential = credential("organization-1").unwrap();
        credential.expires_at = None;
        assert!(!credential.is_valid_at(99));
        assert!(credential.is_valid_at(100 + 7 * 24 * 60 * 60));
        assert!(credential.is_valid_at(u64::MAX));
        credential.revoke(150, Initiator::LocalOperator).unwrap();
        assert!(!credential.is_valid_at(u64::MAX));
    }

    #[test]
    fn credential_is_valid_only_inside_its_unrevoked_lifetime() {
        let mut credential = credential("organization-1").unwrap();
        assert!(!credential.is_valid_at(99));
        assert!(credential.is_valid_at(100));
        assert!(credential.is_valid_at(199));
        assert!(!credential.is_valid_at(200));

        credential.revoke(150, Initiator::LocalOperator).unwrap();
        assert!(!credential.is_valid_at(149));
    }

    #[test]
    fn repeated_revocation_preserves_the_first_recorded_time_and_evidence() {
        let mut credential = credential("organization-1").unwrap();
        let first = credential
            .revoke(250, Initiator::LocalOperator)
            .unwrap()
            .expect("first revocation yields evidence");
        assert_eq!(first.before().unwrap().revoked_at, None);
        assert_eq!(first.after().revoked_at, Some(250));
        assert_eq!(
            first.cause(),
            &TransitionCause::Revoked(RevocationCause::Explicit)
        );
        assert_eq!(first.at(), 250);
        assert_eq!(credential.revoke(0, Initiator::LocalOperator), Ok(None));
        assert_eq!(credential.revoked_at(), Some(250));
    }

    #[test]
    fn explicit_revocation_before_issuance_is_refused_with_the_request_time() {
        let mut credential = credential("organization-1").unwrap();
        assert_eq!(
            credential.revoke(99, Initiator::LocalOperator),
            Err(DomainError::RevokedBeforeIssued {
                issued_at: 100,
                revoked_at: 99
            })
        );
        assert_eq!(credential.revoked_at(), None);
    }

    #[test]
    fn supersession_clamps_forward_and_names_its_replacement() {
        let mut credential = credential("organization-1").unwrap();
        let by = CredentialId::new("credential-2").unwrap();
        let initiator = Initiator::Principal(PrincipalId::new("owner").unwrap());
        let evidence = credential
            .supersede(50, by.clone(), Supersession::Provision, initiator.clone())
            .expect("first supersession yields evidence");
        assert_eq!(credential.revoked_at(), Some(100));
        assert_eq!(evidence.after().revoked_at, Some(100));
        assert_eq!(evidence.at(), 50);
        assert_eq!(
            evidence.cause(),
            &TransitionCause::Revoked(RevocationCause::Superseded {
                by,
                kind: Supersession::Provision
            })
        );
        assert_eq!(evidence.initiator(), &initiator);
        let again = CredentialId::new("credential-3").unwrap();
        assert!(credential
            .supersede(300, again, Supersession::OwnerRecovery, initiator)
            .is_none());
        assert_eq!(credential.revoked_at(), Some(100));
    }

    #[test]
    fn issuance_evidence_describes_the_fresh_credential() {
        let credential = credential("organization-1").unwrap();
        let evidence = credential
            .issued(IssuanceCause::Bootstrap, Initiator::LocalOperator)
            .unwrap();
        assert_eq!(evidence.credential_id(), credential.id());
        assert_eq!(evidence.before(), None);
        assert_eq!(evidence.after(), &credential.lifecycle());
        assert_eq!(
            evidence.cause(),
            &TransitionCause::Issued(IssuanceCause::Bootstrap)
        );
        assert_eq!(evidence.at(), 100);
    }

    #[test]
    fn grant_must_belong_to_credential_organization() {
        assert_eq!(
            credential("organization-2"),
            Err(DomainError::GrantOrganizationMismatch)
        );
    }
}
