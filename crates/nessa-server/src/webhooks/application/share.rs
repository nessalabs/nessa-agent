//! A committed webhook shares as a principal, never as a paired receiver.
use super::{AdmissionFailure, AdmitDelivery, RegistrationLookup};
use crate::conversation::application::{
    load_conversation, AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource,
    AccessSubject, ConversationCaller, ConversationError, GrantedShare, OwnerAccess, RequestShape,
    ShareGrantChange, ShareGrantTransition, ShareTarget,
};
use crate::conversation::domain::{Capabilities, Capability, ShareAccess, ToolPolicy};
use nessa_auth::domain::CredentialId;
use nessa_protocol::conversation::domain::ConversationId;

pub struct ShareWebhookConversation<'a> {
    pub registrations: &'a dyn RegistrationLookup,
    pub admission: AdmitDelivery<'a>,
}
impl ShareWebhookConversation<'_> {
    /// Ownership is admitted before private target lookup. Current Auth is held
    /// through the canonical grant commit; receiver authority is never queried.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute(
        &self,
        caller: ConversationCaller,
        id: ConversationId,
        credential: CredentialId,
        capabilities: Capabilities,
        policy: Option<ToolPolicy>,
        expected_policy_revision: Option<u64>,
        at_ms: u64,
    ) -> Result<GrantedShare, ConversationError> {
        caller.actor()?;
        let access =
            ShareAccess::new(capabilities, policy).map_err(|_| ConversationError::InvalidInput)?;
        let conversation = load_conversation(self.admission.conversations, &id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        OwnerAccess::new(self.admission.policy).admit(&conversation, &caller)?;
        let registration = self
            .registrations
            .registration(&credential)
            .await
            .map_err(|_| ConversationError::Metadata)?
            .filter(|r| r.target.credential == credential)
            .ok_or(ConversationError::ShareTargetNotPaired)?;
        let held = self
            .admission
            .hold_target(&registration.target)
            .await
            .map_err(|e| match e {
                AdmissionFailure::Unavailable => ConversationError::Metadata,
                AdmissionFailure::Refused => ConversationError::ShareTargetNotPaired,
            })?;
        // Resolve again under Auth ownership. A registration stopped or replaced
        // while the hold was pending cannot become the target of this grant.
        let current = self
            .registrations
            .registration(&credential)
            .await
            .map_err(|_| ConversationError::Metadata)?
            .filter(|r| r.id == registration.id && r.target == registration.target)
            .ok_or(ConversationError::ShareTargetNotPaired)?;
        let target = &current.target;
        let named = &held.snapshot.credential;
        let ask = AccessAsk {
            subject: AccessSubject::Person {
                organization: &caller.organization_id,
                principal: &caller.principal_id,
            },
            action: AccessAction::Share,
            resource: AccessResource::of(&conversation),
            held: None,
            request: RequestShape::default(),
            target: Some(ShareTarget {
                organization: &target.organization,
                binding_owner: &target.owner,
                grants: named.grants(),
                gateway: self.admission.gateway,
            }),
        };
        match self.admission.policy.evaluate(&ask) {
            AccessDecision::Allow { .. } => {}
            AccessDecision::Deny(AccessDenial::Unavailable) => {
                return Err(ConversationError::Metadata)
            }
            AccessDecision::Deny(_) => return Err(ConversationError::ShareTargetNotPaired),
        }
        let mut denied = false;
        let mut policies = Vec::new();
        for (capability, action) in [
            (Capability::Comment, AccessAction::Comment),
            (Capability::Drive, AccessAction::Drive),
        ] {
            if !capabilities.contains(capability) {
                continue;
            }
            let ask = AccessAsk {
                subject: AccessSubject::WebhookReader {
                    organization: &target.organization,
                    principal: &target.principal,
                    registration_owner: &target.owner,
                },
                action,
                resource: AccessResource::of(&conversation),
                held: Some(capabilities),
                request: RequestShape::default(),
                target: None,
            };
            match self.admission.policy.evaluate(&ask) {
                AccessDecision::Allow { .. } => {}
                AccessDecision::Deny(AccessDenial::Unavailable) => {
                    return Err(ConversationError::Metadata)
                }
                AccessDecision::Deny(AccessDenial::Forbidden { policies: ids }) => {
                    denied = true;
                    policies.extend(ids);
                }
                AccessDecision::Deny(_) => denied = true,
            }
        }
        if denied {
            policies.sort();
            policies.dedup();
            return Err(ConversationError::CapabilitiesRefused { policies });
        }
        if !named.is_valid_at(self.admission.clock.unix_seconds()) {
            return Err(ConversationError::ShareTargetNotPaired);
        }
        let receipt = self
            .admission
            .grants
            .change_with_credential(
                ShareGrantChange {
                    transition: ShareGrantTransition::Grant(access),
                    conversation_id: id,
                    principal_id: Some(target.principal.clone()),
                    receiver_id: None,
                    credential_id: credential,
                    expected_policy_revision,
                    initiator: caller,
                    at_ms,
                },
                held,
            )
            .await?;
        let policy_revision = receipt.policy_revision.ok_or(ConversationError::Metadata)?;
        Ok(GrantedShare {
            applied: receipt.applied,
            policy_revision,
        })
    }
}
