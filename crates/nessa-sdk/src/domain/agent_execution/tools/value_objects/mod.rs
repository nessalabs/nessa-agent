//! Immutable tool identities, sparse updates, observations, and file descriptions.
//! Paths describe provider output; they do not grant filesystem access.
//! ToolContent owns compact text privately; ToolContentView exposes shared
//! inspection while observations account for content and collection storage.
//!
//! ```text
//! ToolCallUpdate --> ToolObservation --> content + locations
//! ```
//!
//! Arrows mean the update produces observed data composed from these values.
mod identity;
mod tool;
pub use identity::ToolCallId;
pub use tool::{
    FileLocation, FilePath, ToolCallUpdate, ToolContent, ToolContentView, ToolKind,
    ToolObservation, ToolStatus,
};
