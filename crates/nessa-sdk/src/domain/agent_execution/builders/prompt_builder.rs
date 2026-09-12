use crate::domain::agent_execution::{
    value_objects::{Prompt, PromptText},
    ExecutionError,
};

/// Composes text in insertion order, preserving bytes and explicit separators.
/// Callers resolve files, templates, or other sources before adding their text.
#[derive(Default)]
pub struct PromptBuilder {
    text: String,
}
impl PromptBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn text(mut self, text: impl AsRef<str>) -> Self {
        self.text.push_str(text.as_ref());
        self
    }
    pub fn build(self) -> Result<Prompt, ExecutionError> {
        PromptText::new(self.text).map(Prompt::new)
    }
}
