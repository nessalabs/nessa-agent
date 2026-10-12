//! The tombstone a deleted conversation keeps, and what a share of it lets
//! its holder do. Its identity, its model and approval choice, and what a
//! list says about it are `nessa_protocol::conversation::domain`, shared with
//! the device.
mod conversation_deletion;
mod share_access;
pub use conversation_deletion::{
    ConversationDeletion, DeletionContradiction, ProviderSessionErasure, ProviderSessionLink,
};
pub use share_access::{
    ShareAccess, ShareAccessError, ShareRole, ToolPolicy, ToolPolicyError, ToolPreset, ToolRules,
};
