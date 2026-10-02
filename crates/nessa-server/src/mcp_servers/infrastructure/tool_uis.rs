use crate::conversation::application::McpToolUis;
use nessa_sdk::domain::{agent_execution::tools::McpTool, mcp_apps::UiResourceUri};
use nessa_sdk::infrastructure::mcp::McpServers;

/// The conversation view's tool UI lookup, answered from the tools each
/// server last listed over the gateway's own connection.
pub struct ListedToolUis(pub McpServers);
impl McpToolUis for ListedToolUis {
    fn resource_uri(&self, call: &McpTool) -> Option<UiResourceUri> {
        self.0.tool_ui(call).map(|ui| ui.resource_uri().clone())
    }
}
