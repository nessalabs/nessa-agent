//! Conversation identity and access ownership; execution state belongs to the SDK.
//! A conversation's summary — its title, last thing said, and when — is a
//! separate value derived from what was said, never part of ownership.
//! A deleted conversation keeps its ownership record and a tombstone, its
//! `ConversationDeletion`, which `Conversation::check_access` refuses callers
//! by from then on. The tombstone also carries how far the deletion got, and
//! the domain decides which progress is possible and which request decided it.
//! The conversation's identity, its summary and the catalogue stream identity
//! are `nessa_protocol::conversation::domain`, read by gateway and device alike.
//! Which commands its agent may run where is its [`CommandPolicy`].
mod command_policy;
mod entities;
mod receiver;
mod value_objects;
pub use command_policy::CommandPolicy;
pub use entities::{Conversation, ConversationRefusal};
pub use receiver::{
    PairedReceiver, ReceiverBinding, ReceiverInitiator, ReceiverIntent, ReceiverTransition,
    ReceiverTransitionError,
};
pub use value_objects::{
    ConversationDeletion, DeletionContradiction, ProviderSessionErasure, ProviderSessionLink,
};

#[cfg(test)]
#[path = "../../../tests/conversation/domain.rs"]
mod tests;
