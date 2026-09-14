//! Preserves prompt text and ordered, attributed system instructions. Callers resolve
//! files and templates before supplying text; this feature has no external effects.
//!
//! ```text
//! attributed text --> SystemPromptBuilder --> SystemPrompt
//! ```
//!
//! Arrows mean assembling contributions into validated immutable instructions.
pub mod builders;
pub mod value_objects;
pub use builders::SystemPromptBuilder;
pub use value_objects::{
    PromptContribution, PromptContributionView, PromptSource, PromptSourceKind, PromptText,
    SystemPrompt,
};
