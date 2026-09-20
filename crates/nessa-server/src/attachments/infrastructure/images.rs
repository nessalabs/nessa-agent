//! Stored bytes for the agent adapter, through the SDK's own image port.
use crate::attachments::application::AttachmentStore;
use nessa_sdk::{
    application::agent_execution::providers::{UserImageError, UserImageFuture, UserImageSource},
    domain::agent_execution::prompts::ImageReference,
};
use std::sync::Arc;

/// Reads a message's image by content. Whether the conversation may refer to
/// it was settled when the message was accepted; the worker that asks here
/// does not know which conversation it serves.
pub struct StoredUserImages {
    store: Arc<dyn AttachmentStore>,
}
impl StoredUserImages {
    pub fn new(store: Arc<dyn AttachmentStore>) -> Self {
        Self { store }
    }
}
impl UserImageSource for StoredUserImages {
    fn read(&self, image: ImageReference) -> UserImageFuture<'_> {
        Box::pin(async move {
            // One byte past the reference's length is enough for the adapter
            // to see that longer bytes are not the image, without reading them all.
            match self.store.read(image.digest(), image.size() + 1).await {
                Ok(Some(bytes)) => Ok(bytes),
                Ok(None) => Err(UserImageError::Missing),
                Err(_) => Err(UserImageError::Unavailable),
            }
        })
    }
}
