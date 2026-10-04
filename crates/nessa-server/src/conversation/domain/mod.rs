//! Conversation identity and access ownership; execution state belongs to the SDK.
//! A conversation's summary — its title, last thing said, and when — is a
//! separate value derived from what was said, never part of ownership.
//! A deleted conversation keeps its ownership record and a tombstone, its
//! `ConversationDeletion`, which `Conversation::check_access` refuses callers
//! by from then on. The tombstone also carries how far the deletion got, and
//! the domain decides which progress is possible and which request decided it.
//! Catalogue stream identity is pure owner evidence shared by application
//! correlation and the physical metadata source.
mod catalogue_identity;
mod entities;
mod receiver;
mod value_objects;
pub use catalogue_identity::conversation_catalogue_stream;
pub use entities::{Conversation, ConversationRefusal};
pub use receiver::{
    PairedReceiver, ReceiverBinding, ReceiverInitiator, ReceiverIntent, ReceiverTransition,
    ReceiverTransitionError,
};
pub use value_objects::{
    ConversationApprovalMode, ConversationDeletion, ConversationId, ConversationModelId,
    ConversationPreview, ConversationSummary, ConversationTitle, DeletionContradiction,
    ProviderSessionErasure, ProviderSessionLink, LATEST_TIME_MS,
};

#[cfg(test)]
#[path = "../../../tests/conversation/domain.rs"]
mod tests;
