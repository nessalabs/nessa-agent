use crate::domain::agent_execution::ExecutionError;

/// Opaque execution ID. Identity is preserved exactly; no provider format is assumed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExecutionId(String);
impl ExecutionId {
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("execution ID"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque tool call ID. Identity is preserved exactly; no provider format is assumed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ToolCallId(String);
impl ToolCallId {
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("tool call ID"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque permission ID. Identity is preserved exactly; no provider format is assumed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PermissionId(String);
impl PermissionId {
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("permission ID"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque permission option ID. Identity is preserved exactly; no provider format is assumed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PermissionOptionId(String);
impl PermissionOptionId {
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("permission option ID"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
