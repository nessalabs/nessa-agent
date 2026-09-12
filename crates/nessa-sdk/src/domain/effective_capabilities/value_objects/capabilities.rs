use crate::domain::common::value_objects::TokenLimits;
use crate::domain::model_metadata::{
    entities::ModelMetadata,
    value_objects::{Modalities, ModelFeatures, ModelKey},
};
use std::{error::Error, fmt};

/// Binding declarations are ceilings, never overrides of model facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingRestrictions {
    features: ModelFeatures,
    limits: TokenLimits,
}
impl BindingRestrictions {
    pub fn new(features: ModelFeatures, limits: TokenLimits) -> Self {
        Self { features, limits }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modality {
    Text,
    Image,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityRequirement {
    Input(Modality),
    Output(Modality),
    ToolUse,
    Reasoning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityError {
    ConfiguredLimitExceeded {
        limit: &'static str,
        configured: u32,
        maximum: u32,
    },
    NoInputModality,
    NoOutputModality,
    Unsupported(CapabilityRequirement),
    InvalidOutputBudget {
        requested: u32,
        maximum: u32,
    },
    ContextWindowExceeded {
        input_tokens: u64,
        output_tokens: u32,
        maximum: u32,
    },
}
impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfiguredLimitExceeded { limit, configured, maximum } => write!(f, "configured {limit} {configured} exceeds execution ceiling {maximum}"),
            Self::NoInputModality => write!(f, "model and binding share no input modality"),
            Self::NoOutputModality => write!(f, "model and binding share no output modality"),
            Self::Unsupported(requirement) => write!(f, "unsupported capability: {requirement:?}"),
            Self::InvalidOutputBudget { requested, maximum } => write!(f, "output budget {requested} must be positive and at most {maximum}"),
            Self::ContextWindowExceeded { input_tokens, output_tokens, maximum } => write!(f, "input tokens {input_tokens} plus output budget {output_tokens} exceed context window {maximum}"),
        }
    }
}
impl Error for CapabilityError {}

/// An owned snapshot for one model and execution configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveCapabilities {
    model: ModelKey,
    features: ModelFeatures,
    limits: TokenLimits,
}
impl EffectiveCapabilities {
    /// Binding and model ceilings intersect; explicit configuration must fit.
    pub fn new(
        model: &ModelMetadata,
        binding: BindingRestrictions,
        configured_limits: TokenLimits,
    ) -> Result<Self, CapabilityError> {
        let published = model.features();
        let input = intersect(published.input(), binding.features.input())
            .ok_or(CapabilityError::NoInputModality)?;
        let output = intersect(published.output(), binding.features.output())
            .ok_or(CapabilityError::NoOutputModality)?;
        for (limit, configured, maximum) in [
            (
                "context window",
                configured_limits.max_context_window(),
                model
                    .limits()
                    .max_context_window()
                    .min(binding.limits.max_context_window()),
            ),
            (
                "output",
                configured_limits.max_output(),
                model.limits().max_output().min(binding.limits.max_output()),
            ),
        ] {
            if configured > maximum {
                return Err(CapabilityError::ConfiguredLimitExceeded {
                    limit,
                    configured,
                    maximum,
                });
            }
        }
        Ok(Self {
            model: model.key().clone(),
            features: ModelFeatures::new(
                input,
                output,
                published.tool_use() && binding.features.tool_use(),
                published.reasoning() && binding.features.reasoning(),
            ),
            limits: configured_limits,
        })
    }
    pub fn model(&self) -> &ModelKey {
        &self.model
    }
    pub fn features(&self) -> ModelFeatures {
        self.features
    }
    pub fn limits(&self) -> TokenLimits {
        self.limits
    }
    pub fn supports(&self, requirement: CapabilityRequirement) -> bool {
        match requirement {
            CapabilityRequirement::Input(modality) => supports(self.features.input(), modality),
            CapabilityRequirement::Output(modality) => supports(self.features.output(), modality),
            CapabilityRequirement::ToolUse => self.features.tool_use(),
            CapabilityRequirement::Reasoning => self.features.reasoning(),
        }
    }
    /// Validate requirements and a token budget before accepting input.
    /// The caller supplies the total context input count (including history/tools)
    /// and reserves output, including reasoning where applicable. This does not
    /// tokenize content or infer provider-specific accounting.
    pub fn validate(
        &self,
        requirements: &[CapabilityRequirement],
        input_tokens: u64,
        output_tokens: u32,
    ) -> Result<(), CapabilityError> {
        for requirement in requirements {
            if !self.supports(*requirement) {
                return Err(CapabilityError::Unsupported(*requirement));
            }
        }
        if output_tokens == 0 || output_tokens > self.limits.max_output() {
            return Err(CapabilityError::InvalidOutputBudget {
                requested: output_tokens,
                maximum: self.limits.max_output(),
            });
        }
        if u128::from(input_tokens) + u128::from(output_tokens)
            > u128::from(self.limits.max_context_window())
        {
            return Err(CapabilityError::ContextWindowExceeded {
                input_tokens,
                output_tokens,
                maximum: self.limits.max_context_window(),
            });
        }
        Ok(())
    }
}
fn intersect(a: Modalities, b: Modalities) -> Option<Modalities> {
    Modalities::new(
        a.text() && b.text(),
        a.image() && b.image(),
        a.audio() && b.audio(),
    )
    .ok()
}
fn supports(modalities: Modalities, modality: Modality) -> bool {
    match modality {
        Modality::Text => modalities.text(),
        Modality::Image => modalities.image(),
        Modality::Audio => modalities.audio(),
    }
}
