//! Explicit DTO/domain translation; constructors remain the only invariant owners.
use super::dto::{EffectiveCapabilitiesDto, ImageInputLimitsDto, ModalitiesDto, ModelMetadataDto};
use crate::domain::common::value_objects::TokenLimits;
use crate::domain::common::value_objects::{Date, ImageMediaType, Url};
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::domain::model_metadata::{
    entities::ModelMetadata,
    value_objects::ImageInputLimits,
    value_objects::Modalities,
    value_objects::ModelDescription,
    value_objects::ModelFeatures,
    value_objects::{ModelKey, ModelProvider},
    MetadataError,
};

impl TryFrom<ModalitiesDto> for Modalities {
    type Error = MetadataError;
    fn try_from(dto: ModalitiesDto) -> Result<Self, Self::Error> {
        Self::new(dto.text, dto.image, dto.audio)
    }
}
impl From<Modalities> for ModalitiesDto {
    fn from(value: Modalities) -> Self {
        Self {
            text: value.text(),
            image: value.image(),
            audio: value.audio(),
        }
    }
}
impl From<&EffectiveCapabilities> for EffectiveCapabilitiesDto {
    fn from(snapshot: &EffectiveCapabilities) -> Self {
        Self {
            provider: snapshot.model().provider().as_str().into(),
            model_id: snapshot.model().model_id().into(),
            input: snapshot.features().input().into(),
            output: snapshot.features().output().into(),
            tool_use: snapshot.features().tool_use(),
            reasoning: snapshot.features().reasoning(),
            context_window_tokens: snapshot.limits().max_context_window(),
            max_output_tokens: snapshot.limits().max_output(),
        }
    }
}
impl TryFrom<ModelMetadataDto> for ModelMetadata {
    type Error = MetadataError;
    fn try_from(dto: ModelMetadataDto) -> Result<Self, Self::Error> {
        let image_input = dto
            .image_input
            .map(|limits| {
                ImageInputLimits::new(
                    limits
                        .media_types
                        .iter()
                        .map(|media_type| {
                            ImageMediaType::parse(media_type).map_err(|_| MetadataError::Invalid {
                                field: "image input",
                                reason: "unsupported media type",
                            })
                        })
                        .collect::<Result<_, _>>()?,
                    limits.max_encoded_bytes,
                    limits.max_edge_px,
                    limits.many_images_max_edge_px,
                    limits.native_long_edge_px,
                )
            })
            .transpose()?;
        let model = Self::new(
            ModelKey::new(
                ModelProvider::try_from(dto.provider.as_str())?,
                dto.model_id,
            )?,
            ModelDescription::new(
                dto.display_name,
                Date::new(dto.knowledge_cutoff)?,
                Url::new(dto.documentation_url)?,
            )?,
            ModelFeatures::new(
                dto.input.try_into()?,
                dto.output.try_into()?,
                dto.tool_use,
                dto.reasoning,
            ),
            TokenLimits::new(dto.max_context_window_tokens, dto.max_output_tokens)?,
        );
        match image_input {
            Some(limits) => model.with_image_input(limits),
            None => Ok(model),
        }
    }
}
impl From<&ModelMetadata> for ModelMetadataDto {
    fn from(model: &ModelMetadata) -> Self {
        Self {
            provider: model.key().provider().as_str().into(),
            model_id: model.key().model_id().into(),
            display_name: model.description().display_name().into(),
            knowledge_cutoff: model.description().knowledge_cutoff().as_str().into(),
            documentation_url: model.description().documentation_url().as_str().into(),
            input: model.features().input().into(),
            image_input: model.image_input().map(|limits| ImageInputLimitsDto {
                media_types: limits
                    .media_types()
                    .iter()
                    .map(|media_type| media_type.as_str().into())
                    .collect(),
                max_encoded_bytes: limits.max_encoded_bytes(),
                max_edge_px: limits.max_edge_px(),
                many_images_max_edge_px: limits.many_images_max_edge_px(),
                native_long_edge_px: limits.native_long_edge_px(),
            }),
            output: model.features().output().into(),
            tool_use: model.features().tool_use(),
            reasoning: model.features().reasoning(),
            max_context_window_tokens: model.limits().max_context_window(),
            max_output_tokens: model.limits().max_output(),
        }
    }
}
