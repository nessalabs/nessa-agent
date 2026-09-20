//! Fits an uploaded image to the selected model, through `nessa-images`.
use crate::attachments::application::{
    ImageNormalizer, NormalizeError, NormalizeFuture, NormalizedImage,
};
use nessa_images::{normalize, Encoding, Error, Limits, LimitsError};
use nessa_sdk::domain::{
    agent_execution::prompts::ImageReference, common::value_objects::ImageMediaType,
    model_metadata::value_objects::ImageInputLimits,
};

/// The selected model's image limits, applied to every upload that says it is
/// an image.
///
/// The numbers are the model catalog's and nowhere else's. This adapter only
/// translates them: the catalog counts base64 text because providers do, the
/// library counts stored bytes; and the edge worth sending is the smaller of
/// what the model sees and what a long conversation's images are held to. A
/// message's own per-image ceiling stays absolute whatever a model allows.
pub struct ModelImageNormalizer {
    limits: Option<Limits>,
}
impl ModelImageNormalizer {
    /// `None` is a model with no recorded image limits. It is offered no images,
    /// so none is prepared for it: every image upload is refused.
    pub fn new(model: Option<&ImageInputLimits>) -> Result<Self, LimitsError> {
        let limits = model
            .map(|model| {
                Limits::new(
                    model
                        .media_types()
                        .iter()
                        .map(|media_type| match media_type {
                            ImageMediaType::Png => Encoding::Png,
                            ImageMediaType::Jpeg => Encoding::Jpeg,
                            ImageMediaType::Gif => Encoding::Gif,
                            ImageMediaType::Webp => Encoding::Webp,
                        })
                        .collect(),
                    model.max_raw_bytes().min(ImageReference::MAX_BYTES),
                    model.target_long_edge_px(),
                )
            })
            .transpose()?;
        Ok(Self { limits })
    }
}
impl ImageNormalizer for ModelImageNormalizer {
    // The declared media type is deliberately unused: what an upload is gets
    // read from its bytes, never from what the client called it.
    fn normalize<'a>(&'a self, original: Vec<u8>, _: &'a str) -> NormalizeFuture<'a> {
        Box::pin(async move {
            // Nothing is wrong with the upload: this model is offered no
            // images, and saying the image could not be read would be false.
            let limits = self.limits.clone().ok_or(NormalizeError::NotOffered)?;
            // Decoding and encoding are CPU work measured in hundreds of
            // milliseconds. A panic in a decoder is this upload's failure only.
            let fitted = tokio::task::spawn_blocking(move || normalize(&original, &limits))
                .await
                .map_err(|_| NormalizeError::Failed)?
                .map_err(|error| match error {
                    Error::UnsupportedEncoding | Error::Undecodable => NormalizeError::Unsupported,
                    Error::TooLargeToDecode | Error::CannotFit => NormalizeError::TooLarge,
                })?;
            Ok(NormalizedImage {
                media_type: fitted.encoding.media_type().into(),
                bytes: fitted.bytes,
            })
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/attachments/normalizer.rs"]
mod tests;
