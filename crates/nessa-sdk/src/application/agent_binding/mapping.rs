use super::{BindingError, PermissionAnswer, PromptRequest};
use crate::domain::agent_execution::value_objects::{
    ExecutionId, PermissionId, PermissionOptionId,
};

/// The content is already a valid domain Prompt; map execution metadata separately.
impl PromptRequest {
    pub fn execution_id(&self) -> Result<ExecutionId, BindingError> {
        ExecutionId::new(self.execution_id.clone())
            .map_err(|error| BindingError::InvalidInput(error.to_string()))
    }
}
impl PermissionAnswer {
    pub fn to_domain(
        &self,
    ) -> Result<(ExecutionId, PermissionId, PermissionOptionId), BindingError> {
        let execution = ExecutionId::new(self.execution_id.clone())
            .map_err(|error| BindingError::InvalidInput(error.to_string()))?;
        let id = PermissionId::new(self.id.clone())
            .map_err(|error| BindingError::InvalidInput(error.to_string()))?;
        let option_id = PermissionOptionId::new(self.option_id.clone())
            .map_err(|error| BindingError::InvalidInput(error.to_string()))?;
        Ok((execution, id, option_id))
    }
}
