use super::super::{
    value_objects::{ImageInputLimits, ModelDescription, ModelFeatures, ModelKey},
    MetadataError,
};
use crate::domain::common::value_objects::TokenLimits;

/// Model entity, identified by its exact provider/model key.
/// Its metadata is an immutable snapshot constructed from validated value objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelMetadata {
    key: ModelKey,
    description: ModelDescription,
    features: ModelFeatures,
    limits: TokenLimits,
    image_input: Option<ImageInputLimits>,
}
impl ModelMetadata {
    pub fn new(
        key: ModelKey,
        description: ModelDescription,
        features: ModelFeatures,
        limits: TokenLimits,
    ) -> Self {
        Self {
            key,
            description,
            features,
            limits,
            image_input: None,
        }
    }
    /// The same model with its published image input limits. A model whose
    /// features list no image input cannot have limits for it.
    pub fn with_image_input(self, limits: ImageInputLimits) -> Result<Self, MetadataError> {
        if !self.features.input().image() {
            return Err(MetadataError::Invalid {
                field: "image input",
                reason: "limits require the image input modality",
            });
        }
        Ok(Self {
            image_input: Some(limits),
            ..self
        })
    }
    pub fn key(&self) -> &ModelKey {
        &self.key
    }
    pub fn description(&self) -> &ModelDescription {
        &self.description
    }
    pub fn features(&self) -> ModelFeatures {
        self.features
    }
    pub fn limits(&self) -> TokenLimits {
        self.limits
    }
    /// Published limits for one input image. `None` for a model that takes no
    /// images, and for one whose limits are not recorded: nothing can prepare
    /// an image for that model, so it is offered none.
    pub fn image_input(&self) -> Option<&ImageInputLimits> {
        self.image_input.as_ref()
    }
}
