//! Preserves prompt text and ordered, attributed system instructions. Callers resolve
//! files and templates before supplying text; this feature has no external effects.
//!
//! ```text
//! attributed text --> SystemPromptBuilder --> SystemPrompt
//! ```
//!
//! Arrows mean assembling contributions into validated immutable instructions.
//! A `UserMessage` is the other thing said to an agent: one turn's text, the
//! images it refers to by digest, and the files it points at by path.
pub mod builders;
pub mod value_objects;
pub use builders::SystemPromptBuilder;
pub use value_objects::{
    AppModelContext, ImageReference, LinkedFile, McpAppSource, MessageSender, PromptContribution,
    PromptContributionView, PromptSource, PromptSourceKind, PromptText, SystemPrompt, UserMessage,
};
