#![deny(missing_docs)]

use crate::domain::agent_execution::{
    executions::ExecutionId,
    tools::{ToolCallId, ToolCallUpdate, ToolObservation},
    ExecutionError,
};

/// The observed state of one tool owned by an active execution session.
/// Borrow it through [`ExecutionSession::tool`](crate::domain::agent_execution::sessions::ExecutionSession::tool).
/// Only the session constructs or updates entities; callers may clone the immutable
/// [`ToolObservation`] value to retain a snapshot. This entity does not run the tool.
///
/// Tools cannot be independently constructed outside the session boundary:
///
/// ```compile_fail
/// use nessa_sdk::domain::agent_execution::{executions::ExecutionId, tools::{ToolCall, ToolCallUpdate}};
/// fn detached(execution: ExecutionId, update: ToolCallUpdate) -> ToolCall {
///     ToolCall::new(execution, update)
/// }
/// ```
///
/// Nor can a borrowed entity be cloned into a competing mutable entity:
///
/// ```compile_fail
/// use nessa_sdk::domain::agent_execution::tools::ToolCall;
/// fn detached(tool: &ToolCall) -> ToolCall {
///     tool.clone()
/// }
/// ```
///
/// Tool updates must pass through the session's execution and closure checks:
///
/// ```compile_fail
/// use nessa_sdk::domain::agent_execution::{executions::ExecutionId, tools::{ToolCall, ToolCallUpdate}};
/// fn bypass_session(tool: &mut ToolCall, execution: &ExecutionId, update: ToolCallUpdate) {
///     tool.apply(execution, update).unwrap();
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct ToolCall {
    id: ToolCallId,
    execution_id: ExecutionId,
    observation: ToolObservation,
}
impl ToolCall {
    /// Create an observed tool from its owning execution and first sparse update. No tool is executed.
    pub(in crate::domain::agent_execution) fn new(
        execution_id: ExecutionId,
        update: ToolCallUpdate,
    ) -> Self {
        Self {
            id: update.id().clone(),
            execution_id,
            observation: ToolObservation::default().with_update(update),
        }
    }
    /// Identity selected from the first provider update; stable for this entity.
    pub fn id(&self) -> &ToolCallId {
        &self.id
    }
    /// Execution that owns this tool and all subsequent observations.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Borrow the immutable accumulated observation; session updates replace it atomically.
    pub fn observation(&self) -> &ToolObservation {
        &self.observation
    }
    /// Retained variable payload bytes, including the execution and tool identities.
    /// Fixed struct/map storage is bounded separately by the tool count.
    pub fn payload_bytes(&self) -> usize {
        self.execution_id
            .as_str()
            .len()
            .saturating_add(self.id.as_str().len())
            .saturating_add(self.observation.payload_bytes())
    }
    /// Retained payload after applying a sparse update, without copying it.
    pub fn payload_bytes_after(&self, update: &ToolCallUpdate) -> usize {
        self.execution_id
            .as_str()
            .len()
            .saturating_add(self.id.as_str().len())
            .saturating_add(self.observation.payload_bytes_after(update))
    }
    /// Predict retained variable bytes before creating a tool, including both
    /// identity allocations and the first observation, without copying payloads.
    pub fn initial_payload_bytes(execution_id: &ExecutionId, update: &ToolCallUpdate) -> usize {
        execution_id
            .as_str()
            .len()
            .saturating_add(update.id().as_str().len())
            .saturating_add(update.payload_bytes())
    }
    /// Apply the sparse update only when execution and tool identities match. Returns DifferentExecution or DifferentTool without mutation on mismatch.
    pub(in crate::domain::agent_execution) fn apply(
        &mut self,
        execution_id: &ExecutionId,
        update: ToolCallUpdate,
    ) -> Result<(), ExecutionError> {
        if execution_id != &self.execution_id {
            return Err(ExecutionError::DifferentExecution);
        }
        if update.id() != &self.id {
            return Err(ExecutionError::DifferentTool);
        }
        self.observation = std::mem::take(&mut self.observation).with_update(update);
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/tool_identity.rs"]
mod identity_tests;
