use crate::domain::agent_execution::ExecutionError;

/// Opaque execution ID. Identity is preserved exactly; no provider format is assumed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExecutionId(Box<str>);
impl ExecutionId {
    /// Maximum UTF-8 byte length for execution correlation across admission and storage.
    pub const MAX_BYTES: usize = 256;
    /// Preserve exact text; blank input returns EmptyValue and input over MAX_BYTES
    /// returns ValueTooLong. Validation precedes application admission or storage.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("execution ID"));
        }
        if value.len() > Self::MAX_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "execution ID",
                max_bytes: Self::MAX_BYTES,
            });
        }
        // Compact immutable storage makes retained identity bytes equal text length.
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact identity text without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
