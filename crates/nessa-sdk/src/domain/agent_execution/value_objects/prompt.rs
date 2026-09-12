use super::PromptText;

/// Immutable submitted content, reusable across executions and bindings.
/// Execution correlation and token admission counts are request metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    text: PromptText,
}
impl Prompt {
    pub fn new(text: PromptText) -> Self {
        Self { text }
    }
    pub fn text(&self) -> &PromptText {
        &self.text
    }
}
