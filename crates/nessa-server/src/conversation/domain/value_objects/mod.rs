//! Validated immutable conversation identities, what a list says about one,
//! and the tombstone a deleted one keeps.
mod conversation_deletion;
mod conversation_id;
mod conversation_summary;
pub use conversation_deletion::{
    ConversationDeletion, DeletionContradiction, ProviderSessionErasure, ProviderSessionLink,
};
pub use conversation_id::ConversationId;
pub use conversation_summary::{
    ConversationPreview, ConversationSummary, ConversationTitle, LATEST_TIME_MS,
};
