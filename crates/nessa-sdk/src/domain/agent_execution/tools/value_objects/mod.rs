//! Immutable tool identities, sparse updates, observations, and file descriptions.
//! Paths describe provider output; they do not grant filesystem access.
//! ToolContent owns compact text privately; ToolContentView exposes shared
//! inspection while observations account for content and collection storage.
//! McpTool names the MCP server and tool a call went to; it reaches no server.
//! A structured result is content like text, kept as bounded JSON text.
//!
//! ```text
//! ToolCallUpdate --> ToolObservation --> content + locations + MCP identity
//! ```
//!
//! Arrows mean the update produces observed data composed from these values.
mod identity;
pub(in crate::domain::agent_execution) mod json;
mod mcp;
mod tool;
pub use identity::ToolCallId;
pub use mcp::{McpTool, MAX_MCP_NAME_BYTES};
pub(crate) use tool::ToolObservationUndo;
pub use tool::{
    FileLocation, FilePath, ToolCallUpdate, ToolContent, ToolContentView, ToolKind,
    ToolObservation, ToolStatus, MAX_STRUCTURED_RESULT_BYTES,
};
