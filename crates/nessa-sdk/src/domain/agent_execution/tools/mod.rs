//! Tracks tools observed during an execution without executing them. Sparse updates
//! become immutable observations through the identity-bearing tool entity.
//!
//! ```text
//! ToolCallUpdate --> ToolCall --> ToolObservation
//! ```
//!
//! Arrows mean applying an update and exposing the resulting observation.
pub mod entities;
pub mod value_objects;
pub use entities::ToolCall;
pub(crate) use value_objects::ToolObservationUndo;
pub use value_objects::{
    FileLocation, FilePath, McpCallArguments, McpTool, ToolCallId, ToolCallUpdate, ToolContent,
    ToolContentView, ToolKind, ToolObservation, ToolStatus, MAX_MCP_ARGUMENTS_BYTES,
    MAX_MCP_NAME_BYTES, MAX_STRUCTURED_RESULT_BYTES,
};
