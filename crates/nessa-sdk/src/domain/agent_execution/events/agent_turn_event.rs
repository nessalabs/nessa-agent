use crate::domain::agent_execution::value_objects::{
    ExecutionId, FileToolInput, MessageChunk, PermissionId, PermissionOptions, PromptOutcome,
    ToolCallUpdate,
};

/// An immutable observation correlated to one execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTurnEvent {
    execution_id: ExecutionId,
    update: AgentTurnUpdate,
}
impl AgentTurnEvent {
    pub fn new(execution_id: ExecutionId, update: AgentTurnUpdate) -> Self {
        Self {
            execution_id,
            update,
        }
    }
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    pub fn update(&self) -> &AgentTurnUpdate {
        &self.update
    }
    pub fn into_update(self) -> AgentTurnUpdate {
        self.update
    }
}

/// Domain observations contain no provider, transport, or cleanup error types.
/// Finished follows the execution's output; delivery failures are port errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentTurnUpdate {
    Finished(PromptOutcome),
    Message(MessageChunk),
    Tool(ToolCallUpdate),
    PermissionRequested {
        id: PermissionId,
        tool: ToolCallUpdate,
        input: Box<FileToolInput>,
        options: PermissionOptions,
    },
}
