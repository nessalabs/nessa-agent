//! Assembles ordered prompt contributions before validating the final system prompt.
//!
//! ```text
//! PromptSource + text --> SystemPromptBuilder --> SystemPrompt
//! ```
//!
//! Arrows mean adding a contribution and building the immutable result.
mod system_prompt_builder;
pub use system_prompt_builder::SystemPromptBuilder;
