//! The gateway's MCP servers (ADR 344). Everything but the HTTP route an MCP
//! App redeems its resource tickets at lives in nessa-conversation's
//! `mcp_servers`, beside the conversations that use them.
#[cfg(unix)]
pub use nessa_conversation::mcp_servers::conversation_tools;
pub use nessa_conversation::mcp_servers::{application, domain, infrastructure};
pub mod entrypoint;
