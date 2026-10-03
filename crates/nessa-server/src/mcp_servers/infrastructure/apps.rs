//! An MCP App's calls, answered by the SDK's `McpServers` on the
//! conversation's own session of each server.
use crate::conversation::application::{McpAppFailure, McpAppFuture, McpApps};
use crate::product_contract::generated::{MCP_APP_CALL_TIMEOUT_MS, MCP_APP_READ_TIMEOUT_MS};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::mcp_apps::{ListedTool, UiResource, UiResourceUri};
use nessa_sdk::infrastructure::mcp::{McpError, McpServers};
use serde_json::Value;
use std::time::Duration;

/// [`McpApps`] over the gateway's MCP sessions.
pub struct SessionApps(pub McpServers);
impl McpApps for SessionApps {
    fn listed_tool(
        &self,
        session: &SessionId,
        server: &str,
        name: &str,
    ) -> Result<Option<ListedTool>, McpAppFailure> {
        self.0.listed_tool(session, server, name).map_err(failure)
    }
    fn call_tool<'a>(
        &'a self,
        session: &'a SessionId,
        server: &'a str,
        name: &'a str,
        arguments: Option<Value>,
    ) -> McpAppFuture<'a, Value> {
        Box::pin(async move {
            self.0
                .call_tool(
                    session,
                    server,
                    name,
                    arguments,
                    Duration::from_millis(MCP_APP_CALL_TIMEOUT_MS),
                )
                .await
                .map_err(failure)
        })
    }
    fn read_resource<'a>(
        &'a self,
        session: &'a SessionId,
        server: &'a str,
        uri: &'a UiResourceUri,
    ) -> McpAppFuture<'a, UiResource> {
        Box::pin(async move {
            self.0
                .read_app_resource(
                    session,
                    server,
                    uri,
                    Duration::from_millis(MCP_APP_READ_TIMEOUT_MS),
                )
                .await
                .map_err(failure)
        })
    }
}

/// What an app's call is told of the SDK's failure.
pub(crate) fn failure(error: McpError) -> McpAppFailure {
    match error {
        McpError::NoSession | McpError::NotConfigured => McpAppFailure::NoSession,
        McpError::Timeout => McpAppFailure::TimedOut,
        McpError::Remote { code, message } => McpAppFailure::Remote { code, message },
        McpError::TooLarge(_) => McpAppFailure::TooLarge,
        McpError::NotAnApp => McpAppFailure::NotAnApp,
        McpError::Malformed(_) | McpError::Handshake(_) => McpAppFailure::Malformed,
        // As many requests waiting as it takes: nothing was sent, and it
        // passes with time.
        McpError::Busy => McpAppFailure::Busy,
        // Ended, gone or stopped: the session cannot answer this call.
        McpError::ServerGone
        | McpError::Closed
        | McpError::Stopped
        | McpError::Start(_)
        | McpError::InvalidConfiguration => McpAppFailure::SessionEnded,
    }
}

#[cfg(test)]
#[path = "../../../tests/mcp_servers/apps.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "../../../tests/mcp_servers/session_apps.rs"]
mod budget_tests;
