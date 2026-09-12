//! Pure construction of prompts from content supplied by callers.
//! Source loading and schema serialization belong outside the domain.
mod prompt_builder;
pub use prompt_builder::PromptBuilder;
