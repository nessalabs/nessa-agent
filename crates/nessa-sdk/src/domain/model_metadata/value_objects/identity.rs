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
///
/// A provider here is who serves the model, which is not the same question as
/// which harness is driving it: Codex is an OpenAI harness, and Opencode
/// reaches several providers' models through one gateway of its own. `Opencode`
/// names that gateway, because the models it serves are identified only within
/// it — `big-pickle` is a name OpenCode Zen gives something, and nothing
/// outside it answers to that name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelProvider {
    OpenAi,
    Anthropic,
    /// OpenCode Zen, the gateway Opencode's own models are served through.
    Opencode,
}
impl ModelProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Opencode => "opencode",
        }
    }
}
impl TryFrom<&str> for ModelProvider {
    type Error = MetadataError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "openai" => Ok(Self::OpenAi),
            "anthropic" => Ok(Self::Anthropic),
            "opencode" => Ok(Self::Opencode),
            _ => Err(MetadataError::UnsupportedProvider(value.into())),
        }
    }
}
