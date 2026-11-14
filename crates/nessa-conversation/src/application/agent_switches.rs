//! Switches of a conversation to another agent. The SDK history owns whether
//! the switch happened (its handoff record); this journal owns the matching
//! committed agent and model, and which agent the next opening runs.
//! `docs/design/conversation-handoffs.md`.
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationId, ConversationModelId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationAgentSwitchState {
    /// Admitted, and not settled: the next opening runs the requested agent.
    Pending,
    /// The history recorded the switch, and the conversation runs the
    /// requested agent from now on.
    Applied,
    /// The history did not record it, and the conversation stays on the
    /// agent it ran on.
    NotApplied,
}

/// One admitted switch, as it was asked for, and where it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationAgentSwitch {
    pub conversation_id: ConversationId,
    pub organization_id: OrganizationId,
    pub request_id: String,
    pub initiator_principal_id: PrincipalId,
    pub initiator_surface_id: String,
    pub prior_agent: AgentId,
    pub prior_model: ConversationModelId,
    pub requested_agent: AgentId,
    pub requested_model: ConversationModelId,
    pub state: ConversationAgentSwitchState,
    pub requested_at_ms: u64,
}
impl ConversationAgentSwitch {
    /// Whether `other` asks for the same switch: a retry of this request.
    pub fn same_request(&self, other: &Self) -> bool {
        self.conversation_id == other.conversation_id
            && self.organization_id == other.organization_id
            && self.request_id == other.request_id
            && self.initiator_principal_id == other.initiator_principal_id
            && self.initiator_surface_id == other.initiator_surface_id
            && self.requested_agent == other.requested_agent
            && self.requested_model == other.requested_model
    }
}
