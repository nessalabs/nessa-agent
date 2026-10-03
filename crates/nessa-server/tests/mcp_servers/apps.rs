//! What an app's call is told of each way the SDK's session can fail (#348).
use super::failure;
use crate::conversation::application::{McpAppError, McpAppFailure};
use nessa_sdk::infrastructure::mcp::McpError;

#[test]
fn each_session_failure_is_the_one_the_app_is_told() {
    for (error, told, code) in [
        (
            McpError::NoSession,
            McpAppFailure::NoSession,
            "mcp_session_unavailable",
        ),
        (
            McpError::NotConfigured,
            McpAppFailure::NoSession,
            "mcp_session_unavailable",
        ),
        (McpError::Timeout, McpAppFailure::TimedOut, "mcp_timed_out"),
        (
            McpError::Remote {
                code: -32602,
                message: "bad".into(),
            },
            McpAppFailure::Remote {
                code: -32602,
                message: "bad".into(),
            },
            "mcp_remote_error",
        ),
        (
            McpError::TooLarge("result"),
            McpAppFailure::TooLarge,
            "mcp_result_too_large",
        ),
        (
            McpError::NotAnApp,
            McpAppFailure::NotAnApp,
            "mcp_app_unknown",
        ),
        (
            McpError::Malformed("x".into()),
            McpAppFailure::Malformed,
            "mcp_remote_error",
        ),
        (
            McpError::Handshake("x".into()),
            McpAppFailure::Malformed,
            "mcp_remote_error",
        ),
        // Too busy to take it: nothing was sent, and it passes with time.
        (
            McpError::Busy,
            McpAppFailure::Busy,
            "temporarily_unavailable",
        ),
        (
            McpError::ServerGone,
            McpAppFailure::SessionEnded,
            "mcp_session_unavailable",
        ),
        (
            McpError::Closed,
            McpAppFailure::SessionEnded,
            "mcp_session_unavailable",
        ),
        (
            McpError::Stopped,
            McpAppFailure::SessionEnded,
            "mcp_session_unavailable",
        ),
        (
            McpError::Start("x".into()),
            McpAppFailure::SessionEnded,
            "mcp_session_unavailable",
        ),
        (
            McpError::InvalidConfiguration,
            McpAppFailure::SessionEnded,
            "mcp_session_unavailable",
        ),
    ] {
        let failed = failure(error);
        assert_eq!(failed, told);
        assert_eq!(McpAppError::from(failed).code().as_str(), code);
    }
}
