//! The session-owned tool entity preserves execution and tool identity while updating
//! observations. Callers borrow entities and may retain immutable observation snapshots.
//!
//! ```text
//! ExecutionSession + sparse update --> ToolCall --> borrowed immutable observation
//! ```
//!
//! Arrows mean session-internal construction or update, then public observation access.
mod tool_call;
pub use tool_call::ToolCall;
