//! Correlated native model choices. The SDK owns native application evidence;
//! this journal owns the matching committed conversation selection.
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_protocol::conversation::view::ConversationModelChangeEffect;
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationModelSelection {
    pub model: ConversationModelId,
    pub approval_mode: ConversationApprovalMode,
    pub effect: ConversationModelChangeEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationModelRequestState {
    Pending,
    Applied,
    NotApplied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationModelApplication {
    Applied(ConversationModelSelection),
    Refused,
    Uncertain,
}

/// The exact admitted intent and its independently observed native outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationModelRequest {
    pub conversation_id: ConversationId,
    pub organization_id: OrganizationId,
    pub request_id: String,
    pub initiator_principal_id: PrincipalId,
    pub initiator_surface_id: String,
    pub prior_identity: ProviderIdentity,
    pub requested_identity: ProviderIdentity,
    pub prior_model: ConversationModelId,
    pub prior_approval_mode: ConversationApprovalMode,
    pub requested_model: ConversationModelId,
    pub state: ConversationModelRequestState,
    pub application: Option<ConversationModelApplication>,
    pub requested_at_ms: u64,
}
