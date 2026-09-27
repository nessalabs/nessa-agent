//! Owns the consistency boundary of one live agent session: sequential executions,
//! observed tools, pending reviews, and permanent closure of that live attachment.
//!
//! ```text
//! ExecutionSession --> ExecutionId + ToolCall + PermissionRequest
//! ```
//!
//! The arrow means the aggregate owns and coordinates that execution state.
pub mod aggregates;
pub mod value_objects;
pub use aggregates::{ExecutionSession, SessionClosureResult};
pub use value_objects::{
    AttachmentCause, ExecutionFinish, ExecutionSessionId, ProviderContext,
    ProviderContextEvidenceError, SessionClosure, SessionId,
};
