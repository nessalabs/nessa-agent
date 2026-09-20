//! Fits an uploaded image to the selected model, through `nessa-images`.
use crate::attachments::application::{
    ImageNormalizer, NormalizeError, NormalizeFuture, NormalizedImage,
};
use nessa_images::{normalize_with, Encoding, Error, Limits, LimitsError, PlatformDecoder};
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
///
/// The system decoder is the one thing here that reads from outside this
/// process — HEIC, AVIF and camera RAW go to the operating system — so it
/// arrives from composition rather than being reached for, and a test can put
/// a decoder that refuses, stalls, or answers nonsense in its place.
pub struct ModelImageNormalizer {
    limits: Option<Limits>,
    platform: Option<&'static dyn PlatformDecoder>,
}
impl ModelImageNormalizer {
    /// Fit uploads to `model`, reading what this process cannot read itself
    /// through `platform`.
    ///
    /// A `model` of `None` records no image limits, so it is offered no images
    /// and none is prepared for it. A `platform` of `None` reads only the
    /// encodings the image library reads itself.
    pub fn new(
        model: Option<&ImageInputLimits>,
        platform: Option<&'static dyn PlatformDecoder>,
    ) -> Result<Self, LimitsError> {
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
        Ok(Self { limits, platform })
    }
}
impl ImageNormalizer for ModelImageNormalizer {
    fn offers_images(&self) -> bool {
        self.limits.is_some()
    }
    fn normalize(&self, original: Vec<u8>) -> NormalizeFuture<'_> {
        Box::pin(async move {
            // Nothing is wrong with the upload: this model is offered no
            // images, and saying the image could not be read would be false.
            let limits = self.limits.clone().ok_or(NormalizeError::NotOffered)?;
            let platform = self.platform;
            // Decoding and encoding are CPU work measured in hundreds of
            // milliseconds. A panic in a decoder is this upload's failure only.
            let fitted =
                tokio::task::spawn_blocking(move || normalize_with(&original, &limits, platform))
                    .await
                    .map_err(|_| NormalizeError::Failed)?
                    .map_err(|error| match error {
                        Error::UnsupportedEncoding | Error::Undecodable => {
                            NormalizeError::Unsupported
                        }
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
