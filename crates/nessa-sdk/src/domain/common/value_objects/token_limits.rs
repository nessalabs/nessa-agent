use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenLimitsError {
    ZeroContextWindow,
    ZeroOutput,
    OutputExceedsContext { context_window: u32, output: u32 },
}
impl fmt::Display for TokenLimitsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroContextWindow => write!(f, "maximum context window: must be positive"),
            Self::ZeroOutput => write!(f, "maximum output: must be positive"),
            Self::OutputExceedsContext {
                context_window,
                output,
            } => write!(
                f,
                "maximum output {output} exceeds maximum context window {context_window}"
            ),
        }
    }
}
impl Error for TokenLimitsError {}

/// Positive token ceilings with output bounded by the context window.
/// Model metadata uses published ceilings; effective capabilities use validated
/// execution limits. The owner determines which window this value describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenLimits {
    max_context_window: u32,
    max_output: u32,
}
impl TokenLimits {
    pub fn new(max_context_window: u32, max_output: u32) -> Result<Self, TokenLimitsError> {
        if max_context_window == 0 {
            return Err(TokenLimitsError::ZeroContextWindow);
        }
        if max_output == 0 {
            return Err(TokenLimitsError::ZeroOutput);
        }
        if max_output > max_context_window {
            return Err(TokenLimitsError::OutputExceedsContext {
                context_window: max_context_window,
                output: max_output,
            });
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
