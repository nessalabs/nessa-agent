//! Immutable identities, message content, and tool descriptions shared by bindings.
//! Constructors protect meaningful constraints; wire size limits stay in adapters.
mod identity;
mod message;
mod permission;
mod tool;
pub use identity::{ExecutionId, PermissionId, PermissionOptionId, ToolCallId};
pub use message::{MessageChunk, PromptOutcome, PromptText};
pub use permission::{
    PermissionConfig, PermissionDecision, PermissionOption, PermissionOptions, PermissionState,
};
pub use tool::{
    FileLocation, FilePath, FileToolInput, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};

mod prompt;
pub use prompt::Prompt;
