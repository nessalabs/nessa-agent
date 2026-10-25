//! Admission consumes current credential and principal-grant facts. Transport
//! preparation and persistence are separate owners; a MAC never grants access.
use crate::conversation::application::{
    load_conversation, AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource,
    AccessSubject, ConversationAccessPolicyEvaluator, ConversationError, ConversationRepository,
    RequestShape, RevisionHold, ShareGrants, ShareTarget,
};
use nessa_auth::application::ports::{
    AccessAdmission, AccessError, AccessHold, AccessSnapshot, Clock,
};
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId, Resource};
use nessa_protocol::conversation::domain::ConversationId;
use std::sync::{Mutex, PoisonError};

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

/// One dispatch owns partial authority separately from its physical metadata
/// future. Cancellation releases both fences permanently; the supervisor still
/// joins that future. This value supplies no policy decision by itself.
pub(crate) struct DispatchAdmission {
    state: Mutex<DispatchState>,
}
#[derive(Default)]
struct DispatchState {
    stopped: bool,
    checked: bool,
    credential: Option<AccessHold>,
    grant: Option<RevisionHold>,
}
impl DispatchAdmission {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(DispatchState::default()),
        }
    }
    fn active(&self) -> Result<(), AdmissionFailure> {
        if self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stopped
        {
            Err(AdmissionFailure::Refused)
        } else {
            Ok(())
        }
    }
    fn keep_credential(&self, credential: AccessHold) -> Result<(), AdmissionFailure> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.stopped {
            return Err(AdmissionFailure::Refused);
        }
        state.credential = Some(credential);
        Ok(())
    }
    fn keep_grant(&self, grant: RevisionHold) -> Result<(), AdmissionFailure> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.stopped {
            return Err(AdmissionFailure::Refused);
        }
        state.grant = Some(grant);
        Ok(())
    }
    fn checked(&self) -> Result<(), AdmissionFailure> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.stopped || state.credential.is_none() || state.grant.is_none() {
            return Err(AdmissionFailure::Refused);
        }
        state.checked = true;
        Ok(())
    }
    pub(crate) fn cancel(&self) {
        let (credential, grant) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.stopped = true;
            state.checked = false;
            (state.credential.take(), state.grant.take())
        };
        // Reverse acquisition order, with no ownership mutex held while waking writers.
        drop(grant);
        drop(credential);
    }
    pub(crate) fn is_valid_at(&self, now_seconds: u64) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        !state.stopped
            && state.checked
            && state.grant.is_some()
            && state
                .credential
                .as_ref()
                .is_some_and(|held| held.snapshot.credential.is_valid_at(now_seconds))
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
        validate_target_snapshot(
            &credential.snapshot,
            target,
            self.gateway,
            self.clock.unix_seconds(),
        )?;
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
        let grant_hold = self
            .grants
            .hold_principal_revision(conversation, &target.principal, epoch)
            .await
            .map_err(|error| match error {
                ConversationError::Revoked => AdmissionFailure::Refused,
                _ => AdmissionFailure::Unavailable,
            })?;
        self.check_facts(&credential.snapshot, target, conversation, epoch, None)
            .await?;
        Ok(DeliveryHold {
            _credential: credential,
            _grant: grant_hold,
        })
    }

    /// Native dispatch admission: the runtime's deadline can release partial
    /// fences without dropping this future or detaching a physical metadata read.
    pub(crate) async fn execute_dispatch(
        &self,
        attempt: &DispatchAdmission,
        target: &Target,
        conversation: &ConversationId,
        epoch: u64,
    ) -> Result<(), AdmissionFailure> {
        attempt.active()?;
        let credential = self.hold_target(target).await?;
        let snapshot = credential.snapshot.clone();
        attempt.keep_credential(credential)?;
        let grant = self
            .grants
            .hold_principal_gate(conversation, &target.principal)
            .await
            .map_err(|_| AdmissionFailure::Unavailable)?;
        attempt.keep_grant(grant)?;
        self.check_facts(&snapshot, target, conversation, epoch, Some(attempt))
            .await?;
        attempt.checked()
    }

    async fn check_facts(
        &self,
        snapshot: &AccessSnapshot,
        target: &Target,
        conversation: &ConversationId,
        epoch: u64,
        attempt: Option<&DispatchAdmission>,
    ) -> Result<(), AdmissionFailure> {
        if let Some(attempt) = attempt {
            attempt.active()?;
        }
        let named = &snapshot.credential;
        let grant = self
            .grants
            .principal_grant(conversation, &target.principal)
            .await
            .map_err(|_| AdmissionFailure::Unavailable)?
            .ok_or(AdmissionFailure::Refused)?;
        if let Some(attempt) = attempt {
            attempt.active()?;
        }
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
        if let Some(attempt) = attempt {
            attempt.active()?;
        }
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
            AccessDecision::Allow { .. } if named.is_valid_at(self.clock.unix_seconds()) => Ok(()),
            AccessDecision::Deny(AccessDenial::Unavailable) => Err(AdmissionFailure::Unavailable),
            AccessDecision::Deny(_) | AccessDecision::Allow { .. } => {
                Err(AdmissionFailure::Refused)
            }
        }
    }
}

/// Shared binding check for an already-held current Auth snapshot. This grants
/// no resource permission and acquires no second credential admission fence.
pub(crate) fn validate_target_snapshot(
    snapshot: &AccessSnapshot,
    target: &Target,
    gateway: &Resource,
    now_seconds: u64,
) -> Result<(), AdmissionFailure> {
    let named = &snapshot.credential;
    if named.id() != &target.credential
        || named.principal_id() != &target.principal
        || named.organization_id() != &target.organization
        || named.audience_id().as_str() != gateway.id().as_str()
        || named.organization_id() != gateway.organization_id()
        || snapshot.membership.principal_id() != &target.principal
        || snapshot.membership.organization_id() != &target.organization
        || !snapshot.membership.is_active()
        || !named.is_valid_at(now_seconds)
    {
        return Err(AdmissionFailure::Refused);
    }
    Ok(())
}
