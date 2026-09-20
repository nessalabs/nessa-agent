#![deny(missing_docs)]

use super::PromptText;
use crate::domain::{
    agent_execution::ExecutionError,
    common::value_objects::{ImageMediaType, Sha256Digest},
};

/// One image a user message refers to: what the bytes hash to, how they are
/// encoded, and how many there are. It never holds the bytes, so a message
/// stays small enough to compare, queue, and persist whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageReference {
    digest: Sha256Digest,
    media_type: ImageMediaType,
    size: u64,
}
impl ImageReference {
    /// Largest single image, in bytes.
    pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

    /// Refer to `size` bytes of `media_type` hashing to `digest`. An empty image
    /// is [`ExecutionError::EmptyValue`]; one over [`Self::MAX_BYTES`] is
    /// [`ExecutionError::ValueTooLong`]. Nothing is read: whoever resolves the
    /// reference verifies that the bytes agree with it.
    pub fn new(
        digest: Sha256Digest,
        media_type: ImageMediaType,
        size: u64,
    ) -> Result<Self, ExecutionError> {
        if size == 0 {
            return Err(ExecutionError::EmptyValue("image"));
        }
        if size > Self::MAX_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "image",
                max_bytes: Self::MAX_BYTES as usize,
            });
        }
        Ok(Self {
            digest,
            media_type,
            size,
        })
    }
    /// Digest of the image bytes.
    pub fn digest(&self) -> Sha256Digest {
        self.digest
    }
    /// Encoding of the image bytes.
    pub fn media_type(&self) -> ImageMediaType {
        self.media_type
    }
    /// Length of the image in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }
}

/// What a user said in one turn: text, images, or both, in that order.
///
/// Text is optional because an image alone is a complete message; a message
/// with neither is not one. Image order is the order the user attached them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserMessage {
    text: Option<PromptText>,
    images: Box<[ImageReference]>,
}
impl UserMessage {
    /// Most images in one message.
    pub const MAX_IMAGES: usize = 10;
    /// Most image bytes in one message, across all of its images. Encoded for
    /// a provider they grow by a third, and must still fit one 16 MiB frame.
    pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

    /// Combine optional `text` with `images` in attachment order. Neither is
    /// [`ExecutionError::EmptyValue`]; more than [`Self::MAX_IMAGES`] is
    /// [`ExecutionError::TooManyValues`]; more than [`Self::MAX_IMAGE_BYTES`]
    /// in total is [`ExecutionError::ValueTooLong`]. The same image may appear
    /// twice; each appearance counts toward both budgets.
    pub fn new(
        text: Option<PromptText>,
        images: Vec<ImageReference>,
    ) -> Result<Self, ExecutionError> {
        if text.is_none() && images.is_empty() {
            return Err(ExecutionError::EmptyValue("user message"));
        }
        if images.len() > Self::MAX_IMAGES {
            return Err(ExecutionError::TooManyValues {
                field: "user message images",
                max: Self::MAX_IMAGES,
            });
        }
        // Each size is at most `ImageReference::MAX_BYTES`, so ten cannot overflow.
        if images.iter().map(ImageReference::size).sum::<u64>() > Self::MAX_IMAGE_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "user message images",
                max_bytes: Self::MAX_IMAGE_BYTES as usize,
            });
        }
        Ok(Self {
            text,
            images: images.into_boxed_slice(),
        })
    }
    /// A message of text alone, which cannot fail: the text is already nonblank.
    pub fn text_only(text: PromptText) -> Self {
        Self {
            text: Some(text),
            images: Box::default(),
        }
    }
    /// The text, when the user wrote any.
    pub fn text(&self) -> Option<&PromptText> {
        self.text.as_ref()
    }
    /// The exact text, or the empty string for a message of images alone.
    pub fn text_str(&self) -> &str {
        self.text.as_ref().map_or("", PromptText::as_str)
    }
    /// Images in attachment order; empty for a message of text alone.
    pub fn images(&self) -> &[ImageReference] {
        &self.images
    }
}
