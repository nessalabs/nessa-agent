use crate::domain::agent_execution::ExecutionError;

/// Opaque tool call ID. Identity is preserved exactly; no provider format is assumed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ToolCallId(Box<str>);
impl ToolCallId {
    /// Construct an identity preserving exact text; rejects invalid or empty values
    /// according to this identity type's constraints.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("tool call ID"));
        }
        // Compact immutable storage makes retained identity bytes equal text length.
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact identity text without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
