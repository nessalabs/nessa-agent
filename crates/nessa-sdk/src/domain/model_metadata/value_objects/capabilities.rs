use super::super::{error::invalid, MetadataError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modalities {
    text: bool,
    image: bool,
    audio: bool,
}
impl Modalities {
    pub fn new(text: bool, image: bool, audio: bool) -> Result<Self, MetadataError> {
        if !text && !image && !audio {
            return Err(invalid("modalities", "must support at least one modality"));
        }
        Ok(Self { text, image, audio })
    }
    pub fn text(self) -> bool {
        self.text
    }
    pub fn image(self) -> bool {
        self.image
    }
    pub fn audio(self) -> bool {
        self.audio
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFeatures {
    input: Modalities,
    output: Modalities,
    tool_use: bool,
    reasoning: bool,
}
impl ModelFeatures {
    pub fn new(input: Modalities, output: Modalities, tool_use: bool, reasoning: bool) -> Self {
        Self {
            input,
            output,
            tool_use,
            reasoning,
        }
    }
    pub fn input(self) -> Modalities {
        self.input
    }
    pub fn output(self) -> Modalities {
        self.output
    }
    pub fn tool_use(self) -> bool {
        self.tool_use
    }
    pub fn reasoning(self) -> bool {
        self.reasoning
    }
}

/// Positive token ceilings with output bounded by the context window.
/// Model metadata uses published ceilings; effective capabilities use validated
/// execution limits. The owner determines which window this value describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenLimits {
    max_context_window: u32,
    max_output: u32,
}
impl TokenLimits {
    pub fn new(max_context_window: u32, max_output: u32) -> Result<Self, MetadataError> {
        if max_context_window == 0 {
            return Err(invalid("maximum context window", "must be positive"));
        }
        if max_output == 0 || max_output > max_context_window {
            return Err(invalid(
                "maximum output",
                "must be positive and at most the maximum context window",
            ));
        }
        Ok(Self {
            max_context_window,
            max_output,
        })
    }
    /// Percentage of this instance's maximum context window occupied by the supplied tokens.
    pub fn context_usage_percent(self, used_tokens: u64) -> f64 {
        used_tokens as f64 / f64::from(self.max_context_window) * 100.0
    }

    pub fn max_context_window(self) -> u32 {
        self.max_context_window
    }
    pub fn max_output(self) -> u32 {
        self.max_output
    }
}
