//! Immutable prompt text, source attribution, contributions, and composed instructions.
//!
//! ```text
//! PromptSource + text --> PromptContribution --> SystemPrompt
//! ```
//!
//! Arrows mean consuming contributions into one text backing with source ranges.
//! PromptContributionView borrows each attributed range without copying text.
mod prompt;
pub use prompt::{
    PromptContribution, PromptContributionView, PromptSource, PromptSourceKind, PromptText,
    SystemPrompt,
};
