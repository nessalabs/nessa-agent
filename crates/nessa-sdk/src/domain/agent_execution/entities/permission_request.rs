use crate::domain::agent_execution::{value_objects::*, ExecutionError};

/// One reviewable action, bound to an execution and tool. Resolution is once-only.
/// The host authorizes the caller before asking this entity to record a decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRequest {
    id: PermissionId,
    execution_id: ExecutionId,
    tool_id: ToolCallId,
    input: FileToolInput,
    options: PermissionOptions,
    state: PermissionState,
}
impl PermissionRequest {
    pub fn new(
        id: PermissionId,
        execution_id: ExecutionId,
        tool_id: ToolCallId,
        input: FileToolInput,
        options: PermissionOptions,
    ) -> Self {
        Self {
            id,
            execution_id,
            tool_id,
            input,
            options,
            state: PermissionState::Pending,
        }
    }
    pub fn id(&self) -> &PermissionId {
        &self.id
    }
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    pub fn tool_id(&self) -> &ToolCallId {
        &self.tool_id
    }
    pub fn input(&self) -> &FileToolInput {
        &self.input
    }
    pub fn options(&self) -> &PermissionOptions {
        &self.options
    }
    pub fn state(&self) -> &PermissionState {
        &self.state
    }
    pub fn answer(
        &mut self,
        execution_id: &ExecutionId,
        option_id: &PermissionOptionId,
    ) -> Result<PermissionDecision, ExecutionError> {
        if execution_id != &self.execution_id {
            return Err(ExecutionError::DifferentExecution);
        }
        if self.state != PermissionState::Pending {
            return Err(ExecutionError::PermissionResolved);
        }
        let decision = self
            .options
            .find(option_id)
            .ok_or(ExecutionError::UnknownPermissionOption)?
            .decision();
        self.state = PermissionState::Answered {
            option_id: option_id.clone(),
            decision,
        };
        Ok(decision)
    }
    pub fn cancel(&mut self) -> Result<(), ExecutionError> {
        if self.state != PermissionState::Pending {
            return Err(ExecutionError::PermissionResolved);
        }
        self.state = PermissionState::Cancelled;
        Ok(())
    }
}
