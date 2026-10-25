//! Admission consumes current credential and principal-grant facts. Transport
//! preparation and persistence are separate owners; a MAC never grants access.
use crate::conversation::application::{
    load_conversation, AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource,
    AccessSubject, ConversationAccessPolicyEvaluator, ConversationError, ConversationRepository,
    RequestShape, RevisionHold, ShareGrants, ShareTarget,
};
use nessa_auth::application::ports::{AccessAdmission, AccessError, AccessHold, Clock};
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId, Resource};
use nessa_protocol::conversation::domain::ConversationId;

/// Secret-free identity of a committed registration. Infrastructure supplies
/// this from its private registration row, never from an event payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub credential: CredentialId,
    pub principal: PrincipalId,
    pub organization: OrganizationId,
    pub owner: PrincipalId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionFailure {
    Refused,
    Unavailable,
}

/// Both authorities remain fenced until the bounded attempt completes.
pub struct DeliveryHold {
    _credential: AccessHold,
    _grant: RevisionHold,
}

impl DeliveryHold {
    /// Recheck immediately before dispatch, after its durable Sending receipt.
    /// A hold fences mutation; it does not suspend absolute credential expiry.
    pub fn is_valid_at(&self, now_seconds: u64) -> bool {
        self._credential
            .snapshot
            .credential
            .is_valid_at(now_seconds)
    }
}

pub struct AdmitDelivery<'a> {
    pub credentials: &'a dyn AccessAdmission,
    pub grants: &'a dyn ShareGrants,
    pub conversations: &'a dyn ConversationRepository,
    pub policy: &'a dyn ConversationAccessPolicyEvaluator,
    pub clock: &'a dyn Clock,
    pub gateway: &'a Resource,
}

impl AdmitDelivery<'_> {
    /// Shared current-credential binding check for registered share and send.
    /// No grant or physical-work lock is acquired before this original hold.
    pub(super) async fn hold_target(
        &self,
        target: &Target,
    ) -> Result<AccessHold, AdmissionFailure> {
        let credential = self
            .credentials
            .hold(&target.credential)
            .await
            .map_err(|error| match error {
                AccessError::Unavailable | AccessError::StaleRevision => {
                    AdmissionFailure::Unavailable
                }
                _ => AdmissionFailure::Refused,
            })?;
        let snapshot = &credential.snapshot;
        let named = &snapshot.credential;
        if named.id() != &target.credential
            || named.principal_id() != &target.principal
            || named.organization_id() != &target.organization
            || named.audience_id().as_str() != self.gateway.id().as_str()
            || named.organization_id() != self.gateway.organization_id()
            || snapshot.membership.principal_id() != &target.principal
            || snapshot.membership.organization_id() != &target.organization
            || !snapshot.membership.is_active()
            || !named.is_valid_at(self.clock.unix_seconds())
        {
            return Err(AdmissionFailure::Refused);
        }
        Ok(credential)
    }

    /// Lock order is credential, principal/resource grant, then short metadata
    /// reads. Recheck the grant after locking; never use an enqueued decision.
    pub async fn execute(
        &self,
        target: &Target,
        conversation: &ConversationId,
        epoch: u64,
    ) -> Result<DeliveryHold, AdmissionFailure> {
        let credential = self.hold_target(target).await?;
        let named = &credential.snapshot.credential;
        let grant_hold = self
            .grants
            .hold_principal_revision(conversation, &target.principal, epoch)
            .await
            .map_err(|error| match error {
                ConversationError::Revoked => AdmissionFailure::Refused,
                _ => AdmissionFailure::Unavailable,
            })?;
        let grant = self
            .grants
            .principal_grant(conversation, &target.principal)
            .await
            .map_err(|_| AdmissionFailure::Unavailable)?
            .ok_or(AdmissionFailure::Refused)?;
        if grant.conversation_id != *conversation
            || grant.credential_id != target.credential
            || grant.principal_id != target.principal
            || grant.policy_revision != epoch
            || grant.receiver_id.is_some()
        {
            return Err(AdmissionFailure::Refused);
        }
        let metadata = load_conversation(self.conversations, conversation)
            .await
            .map_err(|_| AdmissionFailure::Unavailable)?
            .ok_or(AdmissionFailure::Refused)?;
        let decision = self.policy.evaluate(&AccessAsk {
            subject: AccessSubject::WebhookReader {
                organization: &target.organization,
                principal: &target.principal,
                registration_owner: &target.owner,
            },
            action: AccessAction::Read,
            resource: AccessResource::of(&metadata),
            held: Some(grant.access.capabilities()),
            request: RequestShape::default(),
            target: Some(ShareTarget {
                organization: &target.organization,
                binding_owner: &target.owner,
                grants: named.grants(),
                gateway: self.gateway,
            }),
        });
        match decision {
            AccessDecision::Allow { .. } if named.is_valid_at(self.clock.unix_seconds()) => {
                Ok(DeliveryHold {
                    _credential: credential,
                    _grant: grant_hold,
                })
            }
            AccessDecision::Deny(AccessDenial::Unavailable) => Err(AdmissionFailure::Unavailable),
            AccessDecision::Deny(_) | AccessDecision::Allow { .. } => {
                Err(AdmissionFailure::Refused)
            }
        }
    }
}
