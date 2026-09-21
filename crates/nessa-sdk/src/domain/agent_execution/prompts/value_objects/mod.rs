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
//! PromptText + ImageReference(Sha256Digest, ImageMediaType, size)
//!             + LinkedFile(absolute path)                --> UserMessage
//! ```
//!
//! A user message refers to its images by digest and never holds their bytes,
//! and to its files by path, which it never opens. The two are separate because
//! an image's bytes travel with the message and a file's stay where they are.
mod prompt;
pub use prompt::{
    PromptContribution, PromptContributionView, PromptSource, PromptSourceKind, PromptText,
    SystemPrompt,
};

mod user_message;
pub use user_message::{ImageReference, LinkedFile, UserMessage};
