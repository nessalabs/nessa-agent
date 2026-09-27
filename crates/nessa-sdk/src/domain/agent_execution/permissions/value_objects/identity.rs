use crate::domain::agent_execution::ExecutionError;

/// Opaque permission ID. Identity is preserved exactly; no provider format is assumed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PermissionId(Box<str>);
impl PermissionId {
    /// Construct an identity preserving exact text; rejects invalid or empty values
    /// according to this identity type's constraints.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("permission ID"));
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact identity text without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque permission option ID. Identity is preserved exactly; no provider format is assumed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PermissionOptionId(Box<str>);
impl PermissionOptionId {
    /// Construct an identity preserving exact text; rejects invalid or empty values
    /// according to this identity type's constraints.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("permission option ID"));
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact identity text without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Host-owned identity; unrelated to a socket, credential, or provider option ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PermissionApplicationId(Box<str>);
impl PermissionApplicationId {
    /// Construct an identity preserving exact text; rejects invalid or empty values
    /// according to this identity type's constraints.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue(
                "permission application boundary ID",
            ));
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact identity text without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Host-owned identity; unrelated to a socket, credential, or provider option ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PermissionSessionId(Box<str>);
impl PermissionSessionId {
    /// Construct an identity preserving exact text; rejects invalid or empty values
    /// according to this identity type's constraints.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("permission session ID"));
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact identity text without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
