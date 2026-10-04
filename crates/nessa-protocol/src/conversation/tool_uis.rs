use nessa_sdk::domain::agent_execution::{sessions::SessionId, tools::McpTool};
use nessa_sdk::domain::mcp_apps::UiResourceUri;

/// The UI an MCP tool declared, as its server last listed it (ADR 344).
///
/// No harness passes a tool's `_meta.ui` through ACP, so the view asks the
/// conversation's own connection to each server, which the gateway holds.
/// Answered from what was last listed, never by asking a server while a view
/// is read.
pub trait McpToolUis: Send + Sync {
    /// The `ui://` resource of the listed tool an observed `call` names
    /// (`ListedTool::ui_for`), as `session`'s own session of the call's
    /// server last listed it, or `None`.
    fn resource_uri(&self, session: &SessionId, call: &McpTool) -> Option<UiResourceUri>;
}

/// No MCP servers, so no tool has a UI.
pub struct NoMcpToolUis;
impl McpToolUis for NoMcpToolUis {
    fn resource_uri(&self, _: &SessionId, _: &McpTool) -> Option<UiResourceUri> {
        None
    }
}
