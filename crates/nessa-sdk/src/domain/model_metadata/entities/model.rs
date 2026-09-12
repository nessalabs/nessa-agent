use super::super::value_objects::{ModelDescription, ModelFeatures, ModelKey};
use crate::domain::common::value_objects::TokenLimits;

/// Model entity, identified by its exact provider/model key.
/// Its metadata is an immutable snapshot constructed from validated value objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelMetadata {
    key: ModelKey,
    description: ModelDescription,
    features: ModelFeatures,
    limits: TokenLimits,
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
        }
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
}
