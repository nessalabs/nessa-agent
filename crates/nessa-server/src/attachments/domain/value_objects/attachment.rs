use super::MediaType;
use crate::attachments::domain::AttachmentError;
use nessa_sdk::domain::{
    agent_execution::prompts::ImageReference, common::value_objects::Sha256Digest,
};

/// One file as three facts that must agree: what its bytes hash to, what it
/// was declared to be, and how long it is. It never holds the bytes.
///
/// Two attachments are the same only when all three agree. A digest alone is
/// not enough: a hold for `image/png` does not answer for the same bytes
/// declared `image/jpeg`, and a matching digest with another size describes
/// bytes that cannot exist.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Attachment {
    digest: Sha256Digest,
    media_type: MediaType,
    size: u64,
}
impl Attachment {
    /// Largest file storage accepts. The prompt's own image budget is smaller.
    pub const MAX_BYTES: u64 = 20 * 1024 * 1024;

    pub fn new(
        digest: Sha256Digest,
        media_type: MediaType,
        size: u64,
    ) -> Result<Self, AttachmentError> {
        if size == 0 || size > Self::MAX_BYTES {
            return Err(AttachmentError::Size);
        }
        Ok(Self {
            digest,
            media_type,
            size,
        })
    }
    /// The file a message's image reference names. An image reference is
    /// already nonempty and smaller than [`Self::MAX_BYTES`].
    pub fn of_image(image: &ImageReference) -> Self {
        Self {
            digest: image.digest(),
            media_type: MediaType::of_image(image.media_type()),
            size: image.size(),
        }
    }
    pub fn digest(&self) -> Sha256Digest {
        self.digest
    }
    pub fn media_type(&self) -> &MediaType {
        &self.media_type
    }
    pub fn size(&self) -> u64 {
        self.size
    }
}
