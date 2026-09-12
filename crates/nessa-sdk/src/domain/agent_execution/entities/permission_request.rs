use crate::domain::agent_execution::{value_objects::*, ExecutionError};

/// One reviewable action, bound to an execution and tool. Resolution is once-only.
/// The host authorizes the caller before asking this entity to record a decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRequest {
    id: PermissionId,
    execution_id: ExecutionId,
    tool_id: ToolCallId,
    input: FileToolInput,
    state: PermissionState,
}
impl PermissionRequest {
    pub fn new(
        id: PermissionId,
        execution_id: ExecutionId,
        tool_id: ToolCallId,
        input: FileToolInput,
    ) -> Self {
        Self {
            id,
            execution_id,
            tool_id,
            input,
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
    pub fn state(&self) -> PermissionState {
        self.state
    }
    pub fn answer(
        &mut self,
        execution_id: &ExecutionId,
        decision: PermissionDecision,
    ) -> Result<(), ExecutionError> {
        if execution_id != &self.execution_id {
            return Err(ExecutionError::DifferentExecution);
        }
        if self.state != PermissionState::Pending {
            return Err(ExecutionError::PermissionResolved);
        }
        self.state = PermissionState::Answered(decision);
        Ok(())
    }
    pub fn cancel(&mut self) -> Result<(), ExecutionError> {
        if self.state != PermissionState::Pending {
            return Err(ExecutionError::PermissionResolved);
        }
        self.state = PermissionState::Cancelled;
        Ok(())
    }
}
