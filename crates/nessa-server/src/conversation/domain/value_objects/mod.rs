//! The tombstone a deleted conversation keeps, what a share of it lets its
//! holder do, and the tool calls a share's policy is asked about. Its
//! identity, its model and approval choice, and what a list says about it
//! are `nessa_protocol::conversation::domain`, shared with the device.
mod conversation_deletion;
mod share_access;
mod tool_call;
pub use conversation_deletion::{
    ConversationDeletion, DeletionContradiction, ProviderSessionErasure, ProviderSessionLink,
};
pub use share_access::{
    CommandHosts, ShareAccess, ShareAccessError, ShareRole, ToolChoice, ToolPolicy,
    ToolPolicyError, ToolPreset, ToolRules,
};
pub use tool_call::{ToolCall, ToolSource};
