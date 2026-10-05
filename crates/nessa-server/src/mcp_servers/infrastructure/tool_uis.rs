use crate::conversation::application::conversation_session;
use nessa_protocol::conversation::{domain::ConversationId, tool_uis::McpToolUis};
use nessa_sdk::domain::{agent_execution::tools::McpTool, mcp_apps::UiResourceUri};
use nessa_sdk::infrastructure::mcp::McpServers;

/// The conversation view's tool UI lookup, answered from the tools the
/// conversation's own session of the server last listed, over the gateway's
/// connection.
pub struct ListedToolUis(pub McpServers);
impl McpToolUis for ListedToolUis {
    fn resource_uri(&self, conversation: &ConversationId, call: &McpTool) -> Option<UiResourceUri> {
        self.0
            .tool_ui(&conversation_session(conversation), call)
            .and_then(|ui| ui.resource_uri().cloned())
    }
}
