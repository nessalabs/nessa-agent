//! Immutable prompt text, source attribution, contributions, and composed instructions.
//!
//! ```text
//! PromptSource + text --> PromptContribution --> SystemPrompt
//! ```
//!
//! Arrows mean consuming contributions into one text backing with source ranges.
//! PromptContributionView borrows each attributed range without copying text.
//!
//! ```text
//! PromptText + ImageReference(Sha256Digest, ImageMediaType, size) --> UserMessage
//! ```
//!
//! A user message refers to its images by digest and never holds their bytes.
mod prompt;
pub use prompt::{
    PromptContribution, PromptContributionView, PromptSource, PromptSourceKind, PromptText,
    SystemPrompt,
};

mod user_message;
pub use user_message::{ImageMediaType, ImageReference, UserMessage};
