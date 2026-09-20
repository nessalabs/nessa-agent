//! Preserves prompt text and ordered, attributed system instructions. Callers resolve
//! files and templates before supplying text; this feature has no external effects.
//!
//! ```text
//! attributed text --> SystemPromptBuilder --> SystemPrompt
//! ```
//!
//! Arrows mean assembling contributions into validated immutable instructions.
//! A `UserMessage` is the other thing said to an agent: one turn's text and the
//! images it refers to by digest.
pub mod builders;
pub mod value_objects;
pub use builders::SystemPromptBuilder;
pub use value_objects::{
    ImageReference, PromptContribution, PromptContributionView, PromptSource, PromptSourceKind,
    PromptText, SystemPrompt, UserMessage,
};
