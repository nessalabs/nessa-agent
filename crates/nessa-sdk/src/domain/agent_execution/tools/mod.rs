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
pub use value_objects::{
    FileLocation, FilePath, ToolCallId, ToolCallUpdate, ToolContent, ToolContentView, ToolKind,
    ToolObservation, ToolStatus,
};
