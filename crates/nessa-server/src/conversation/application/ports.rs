use super::ConversationError;
use crate::conversation::domain::{Conversation, ConversationId};
use std::{future::Future, pin::Pin};

pub type ConversationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ConversationError>> + Send + 'a>>;
/// Stores only conversation ownership. The SDK stores execution history separately.
pub trait ConversationRepository: Send + Sync {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>>;
    /// Create once, or return the existing owner without changing it.
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, Conversation>;
}
