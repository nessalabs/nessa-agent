//! Which SDK session a conversation's agent runs in: the one rule.
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::sessions::SessionId;

/// The SDK session a conversation's agent runs in, named by the
/// conversation's id. Every key the SDK is given for a conversation — its
/// session storage, its provider opens, and so the MCP grants issued for
/// them — and every lookup by one, such as the view's tool UIs, is this.
pub(crate) fn conversation_session(id: &ConversationId) -> SessionId {
    SessionId::new(id.to_string()).expect("a conversation id (a UUID) names an SDK session")
}
