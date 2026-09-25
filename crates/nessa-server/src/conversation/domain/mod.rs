//! Conversation identity and access ownership; execution state belongs to the SDK.
//! A conversation's summary — its title, last thing said, and when — is a
//! separate value derived from what was said, never part of ownership.
//! A deleted conversation keeps its ownership record and a tombstone, its
//! `ConversationDeletion`, which `Conversation::check_access` refuses callers
//! by from then on. The tombstone also carries how far the deletion got, and
//! the domain decides which progress is possible and which request decided it.
mod entities;
mod value_objects;
pub use entities::{Conversation, ConversationRefusal};
pub use value_objects::{
    ConversationDeletion, ConversationId, ConversationPreview, ConversationSummary,
    ConversationTitle, DeletionContradiction, ProviderSessionErasure, ProviderSessionLink,
    LATEST_TIME_MS,
};

#[cfg(test)]
#[path = "../../../tests/conversation/domain.rs"]
mod tests;
