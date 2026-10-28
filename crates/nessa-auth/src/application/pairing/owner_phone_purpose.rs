//! Current delegated purpose; historical bootstrap linkage is not runtime authority.
use crate::{
    application::{
        ports::{AccessError, Clock, PortFuture},
        session::AuthenticatedSession,
    },
    domain::{
        pairing::{ConsentClass, PairingPhase, PairingRecord},
        AudienceId, AuthContext, CredentialId, MembershipId, OrganizationId, PrincipalId,
        ResourceId,
    },
};

/// Canonical Auth enrollment linked to its actual remembered receiver. This
/// projection is not a policy decision or current receiver progress: consumers
/// recheck their current receiver and target on every operation and watch batch.
/// A public class/record/DTO cannot construct this trusted registry projection.
pub struct OwnerPhonePurpose {
    credential: CredentialId,
    receiver: ResourceId,
    paired_epoch: u64,
    owner: PrincipalId,
    organization: OrganizationId,
    membership: MembershipId,
    audience: AudienceId,
}
impl OwnerPhonePurpose {
    pub(crate) fn from_registry(
        record: &PairingRecord,
        context: &AuthContext,
    ) -> Result<Self, AccessError> {
        let intent = record.intent();
        let (receiver, paired_epoch) = record.receiver_binding().ok_or(AccessError::Unavailable)?;
        if intent.class() != ConsentClass::OwnerPhoneReadWrite
            || record.phase() != PairingPhase::Active
            || record.owner_approved_key().is_none()
            || record.credential() != Some(context.credential_id())
            || intent.owner() != context.principal_id()
            || intent.membership() != context.membership_id()
            || intent.audience() != context.audience_id()
            || intent.resource().organization_id() != context.organization_id()
        {
            return Err(AccessError::IdentityMismatch);
        }
        Ok(Self {
            credential: context.credential_id().clone(),
            receiver: receiver.clone(),
            paired_epoch,
            owner: intent.owner().clone(),
            organization: intent.resource().organization_id().clone(),
            membership: intent.membership().clone(),
            audience: intent.audience().clone(),
        })
    }
    /// Exact delegated credential, not the historical bootstrap credential.
    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }
    /// Actual receiver remembered before delegated publication.
    pub fn receiver(&self) -> &ResourceId {
        &self.receiver
    }
    /// Historical Pair receipt epoch; current receiver progress may advance.
    pub fn paired_epoch(&self) -> u64 {
        self.paired_epoch
    }
    /// Owner whose current/future conversations the purpose addresses.
    pub fn owner(&self) -> &PrincipalId {
        &self.owner
    }
    /// Original organization, retained by delegation.
    pub fn organization(&self) -> &OrganizationId {
        &self.organization
    }
    /// Delegated membership, checked currently by Auth.
    pub fn membership(&self) -> &MembershipId {
        &self.membership
    }
    /// Exact deployment of this delegated credential.
    pub fn audience(&self) -> &AudienceId {
        &self.audience
    }
}

/// Current canonical purpose reader. None means an actual non-owner-phone
/// credential; operational/contradictory state never becomes None or owner access.
pub trait OwnerPhonePurposeSource: Send + Sync {
    /// Validate the actual current delegated session and exact canonical proof
    /// linkage from one revision. Bootstrap expiry/retirement is historical after
    /// Active; original setup operations still require that original issuer.
    fn owner_phone_purpose<'a>(
        &'a self,
        session: &'a AuthenticatedSession,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, Option<OwnerPhonePurpose>>;
}
