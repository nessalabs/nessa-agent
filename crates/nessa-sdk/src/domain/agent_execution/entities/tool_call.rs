use crate::domain::agent_execution::{value_objects::*, ExecutionError};

/// The observed state of one tool within an execution. It does not run the tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    execution_id: ExecutionId,
    observation: ToolCallUpdate,
}
impl ToolCall {
    pub fn new(execution_id: ExecutionId, observation: ToolCallUpdate) -> Self {
        Self {
            execution_id,
            observation,
        }
    }
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    pub fn observation(&self) -> &ToolCallUpdate {
        &self.observation
    }
    pub fn apply(
        &mut self,
        execution_id: &ExecutionId,
        update: ToolCallUpdate,
    ) -> Result<(), ExecutionError> {
        if execution_id != &self.execution_id {
            return Err(ExecutionError::DifferentExecution);
        }
        if update.id() != self.observation.id() {
            return Err(ExecutionError::DifferentTool);
        }
        let old = &self.observation;
        self.observation = ToolCallUpdate::new(
            old.id().clone(),
            update.title().clone().or_else(|| old.title().clone()),
            update.kind().or(*old.kind()),
            update.status().or(*old.status()),
            update
                .locations()
                .clone()
                .or_else(|| old.locations().clone()),
            update.content().clone().or_else(|| old.content().clone()),
        );
        Ok(())
    }
}
