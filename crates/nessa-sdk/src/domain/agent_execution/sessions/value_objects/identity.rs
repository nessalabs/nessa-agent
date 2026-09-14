use crate::domain::agent_execution::ExecutionError;

/// Identity of an opened execution context, qualified by its binding instance.
/// A provider may supply the opaque value; it is not a host permission scope or credential.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExecutionSessionId(Box<str>);
impl ExecutionSessionId {
    /// Maximum UTF-8 byte length of a provider context identity retained in lifecycle evidence.
    pub const MAX_BYTES: usize = 256;
    /// Preserve the exact nonblank provider identity, including Unicode and whitespace.
    /// Returns `EmptyValue` for blank input or `ValueTooLong` above `MAX_BYTES` UTF-8
    /// bytes. Compact immutable storage retains only the accepted text, including
    /// when the caller transfers a string with excess allocation capacity.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("execution session ID"));
        }
        if value.len() > Self::MAX_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "execution session ID",
                max_bytes: Self::MAX_BYTES,
            });
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact identity text without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Portable local conversation key, distinct from the provider's opaque context ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(Box<str>);
impl SessionId {
    /// Maximum byte length of a portable local conversation key.
    pub const MAX_BYTES: usize = 128;
    /// Preserve `value` as a nonempty key of at most `MAX_BYTES` ASCII letters,
    /// digits, underscores, or hyphens. Other input returns `InvalidSessionId`;
    /// Unicode and whitespace are not portable key characters. Compact immutable
    /// storage discards excess capacity transferred by an owned input string.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > Self::MAX_BYTES
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ExecutionError::InvalidSessionId);
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact identity text without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/session_identity_storage.rs"]
mod tests;
