//! The tombstone a deleted conversation keeps. Its identity, its model and
//! approval choice, and what a list says about it are
//! `nessa_protocol::conversation::domain`, shared with the device.
mod conversation_deletion;
pub use conversation_deletion::{
    ConversationDeletion, DeletionContradiction, ProviderSessionErasure, ProviderSessionLink,
};
