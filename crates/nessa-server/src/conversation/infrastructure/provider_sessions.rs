//! Reaches an agent's own store of sessions through that agent's SDK binding.
//!
//! One adapter for every agent: which agent is asked is the registry's
//! decision, and what an answer means is the binding's. This only carries the
//! binding's answer across, claiming nothing it did not.
use crate::conversation::{
    application::{ConversationError, ConversationFuture, ProviderSessionEraser},
    domain::ProviderSessionErasure,
};
use nessa_sdk::{
    application::agent_execution::providers::{ProviderSessionDeleter, ProviderSessionDeletion},
    domain::agent_execution::sessions::ExecutionSessionId,
};
use std::{future::Future, pin::Pin, sync::Arc};

/// A [`ProviderSessionEraser`] backed by one agent binding's
/// [`ProviderSessionDeleter`].
pub struct BindingSessionEraser {
    binding: Arc<dyn ProviderSessionDeleter>,
}
impl BindingSessionEraser {
    /// Ask `binding` whenever a session of its agent is to be erased.
    pub fn new(binding: Arc<dyn ProviderSessionDeleter>) -> Self {
        Self { binding }
    }
}
impl ProviderSessionEraser for BindingSessionEraser {
    fn erase(&self, session: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        Box::pin(async move {
            let deletion = self
                .binding
                .delete_session(session)
                .await
                .map_err(ConversationError::Agent)?;
            Ok(match deletion {
                ProviderSessionDeletion::Deleted => ProviderSessionErasure::Deleted,
                ProviderSessionDeletion::Archived => ProviderSessionErasure::Archived,
                ProviderSessionDeletion::Acknowledged => ProviderSessionErasure::Acknowledged,
                ProviderSessionDeletion::NotListed => ProviderSessionErasure::NotListed,
                ProviderSessionDeletion::NotSupported => ProviderSessionErasure::NotSupported,
            })
        })
    }
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.binding.settled()
    }
}
