use super::{BindingError, PermissionAnswer, Prompt};
use crate::domain::agent_execution::value_objects::{
    ExecutionId, PermissionDecision, PermissionId, PromptText,
};

/// Boundary DTOs enter the domain through validated values, with no protocol rules.
impl Prompt {
    pub fn to_domain(&self) -> Result<(ExecutionId, PromptText), BindingError> {
        let execution = ExecutionId::new(self.execution_id.clone())
            .map_err(|error| BindingError::InvalidInput(error.to_string()))?;
        let text = PromptText::new(self.text.clone())
            .map_err(|error| BindingError::InvalidInput(error.to_string()))?;
        Ok((execution, text))
    }
}
impl PermissionAnswer {
    pub fn to_domain(
        &self,
    ) -> Result<(ExecutionId, PermissionId, PermissionDecision), BindingError> {
        let execution = ExecutionId::new(self.execution_id.clone())
            .map_err(|error| BindingError::InvalidInput(error.to_string()))?;
        let id = PermissionId::new(self.id.clone())
            .map_err(|error| BindingError::InvalidInput(error.to_string()))?;
        let decision = if self.allow_once {
            PermissionDecision::AllowOnce
        } else {
            PermissionDecision::RejectOnce
        };
        Ok((execution, id, decision))
    }
}
