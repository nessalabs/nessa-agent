use super::super::{error::invalid, MetadataError};

/// Identity is the pair, not a model ID alone. No alias normalization.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelKey {
    provider: ModelProvider,
    model_id: String,
}
impl ModelKey {
    pub fn new(provider: ModelProvider, model_id: String) -> Result<Self, MetadataError> {
        if model_id.is_empty()
            || model_id
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(invalid(
                "model ID",
                "must be a nonempty identifier without whitespace or control characters",
            ));
        }
        Ok(Self { provider, model_id })
    }
    pub fn provider(&self) -> ModelProvider {
        self.provider
    }
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// The closed set of providers supported by the model metadata domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelProvider {
    OpenAi,
    Anthropic,
}
impl ModelProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}
impl TryFrom<&str> for ModelProvider {
    type Error = MetadataError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "openai" => Ok(Self::OpenAi),
            "anthropic" => Ok(Self::Anthropic),
            _ => Err(MetadataError::UnsupportedProvider(value.into())),
        }
    }
}
