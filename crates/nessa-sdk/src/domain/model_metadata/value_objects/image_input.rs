use super::super::MetadataError;
use crate::domain::common::value_objects::ImageMediaType;

fn invalid(reason: &'static str) -> MetadataError {
    MetadataError::Invalid {
        field: "image input",
        reason,
    }
}

/// What a model is published to accept as one input image.
///
/// This is the one place those facts live. Whatever prepares an image for a
/// model (converting its encoding, scaling it down, compressing it) reads them
/// from here rather than carrying its own numbers.
///
/// Every figure is the strictest published across the platforms that serve the
/// model, because an agent process may be pointed at any of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageInputLimits {
    media_types: Vec<ImageMediaType>,
    max_encoded_bytes: u64,
    max_edge_px: u32,
    many_images_max_edge_px: u32,
    native_long_edge_px: u32,
}
impl ImageInputLimits {
    /// - `media_types`: accepted encodings; at least one, none repeated.
    /// - `max_encoded_bytes`: largest single image as base64 text, which is how
    ///   providers measure it.
    /// - `max_edge_px`: longest width or height accepted at all.
    /// - `many_images_max_edge_px`: the lower ceiling a provider applies to every
    ///   image once a request holds many of them. A conversation resends its
    ///   earlier images, so a long one reaches this without any single message
    ///   doing so.
    /// - `native_long_edge_px`: the long edge the model actually sees; anything
    ///   larger is scaled down by the provider, at a cost in bytes and nothing
    ///   gained.
    ///
    /// All are positive, and neither smaller edge exceeds `max_edge_px`.
    pub fn new(
        media_types: Vec<ImageMediaType>,
        max_encoded_bytes: u64,
        max_edge_px: u32,
        many_images_max_edge_px: u32,
        native_long_edge_px: u32,
    ) -> Result<Self, MetadataError> {
        if media_types.is_empty() {
            return Err(invalid("must accept at least one media type"));
        }
        if (1..media_types.len()).any(|index| media_types[..index].contains(&media_types[index])) {
            return Err(invalid("media types must not repeat"));
        }
        if max_encoded_bytes == 0
            || max_edge_px == 0
            || many_images_max_edge_px == 0
            || native_long_edge_px == 0
        {
            return Err(invalid("limits must be positive"));
        }
        if many_images_max_edge_px > max_edge_px || native_long_edge_px > max_edge_px {
            return Err(invalid("no edge limit may exceed the maximum edge"));
        }
        Ok(Self {
            media_types,
            max_encoded_bytes,
            max_edge_px,
            many_images_max_edge_px,
            native_long_edge_px,
        })
    }
    /// Accepted encodings, in published order.
    pub fn media_types(&self) -> &[ImageMediaType] {
        &self.media_types
    }
    /// Largest single image as base64 text.
    pub fn max_encoded_bytes(&self) -> u64 {
        self.max_encoded_bytes
    }
    /// Largest single image in raw bytes: what fits `max_encoded_bytes` once
    /// base64 has turned every three bytes into four characters.
    pub fn max_raw_bytes(&self) -> u64 {
        self.max_encoded_bytes / 4 * 3
    }
    /// Longest width or height accepted at all.
    pub fn max_edge_px(&self) -> u32 {
        self.max_edge_px
    }
    /// Longest edge accepted once a request holds many images.
    pub fn many_images_max_edge_px(&self) -> u32 {
        self.many_images_max_edge_px
    }
    /// The long edge the model actually sees.
    pub fn native_long_edge_px(&self) -> u32 {
        self.native_long_edge_px
    }
    /// The long edge worth sending: all the model sees, and never so large that
    /// a long conversation's images start being refused.
    pub fn target_long_edge_px(&self) -> u32 {
        self.native_long_edge_px.min(self.many_images_max_edge_px)
    }
}
