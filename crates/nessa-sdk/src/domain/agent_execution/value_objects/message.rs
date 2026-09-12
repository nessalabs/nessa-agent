use crate::domain::agent_execution::ExecutionError;

/// Submitted user text must contain something to execute. Preserve its whitespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptText(String);
impl PromptText {
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("prompt text"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An incremental message fragment, not a complete persisted message.
/// Empty and whitespace-only fragments are valid and must retain their meaning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageChunk {
    Text(String),
    Thought(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptOutcome {
    Completed,
    OutputLimit,
    RequestLimit,
    Refused,
    Cancelled,
}
