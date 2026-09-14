//! Once-only evidence of releasing an active execution from its live session.
#![deny(missing_docs)]

use crate::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome},
    permissions::PermissionCancellationReason,
    sessions::ExecutionSessionId,
};

/// Immutable transition from an active execution to no active execution.
/// A successful value retains the reported outcome; a failure retains its lifecycle
/// cause. This local release does not prove that external tools were rolled back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionFinish {
    session_id: ExecutionSessionId,
    execution_id: ExecutionId,
    result: Result<ExecutionOutcome, PermissionCancellationReason>,
}
impl ExecutionFinish {
    pub(crate) fn new(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        result: Result<ExecutionOutcome, PermissionCancellationReason>,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            result,
        }
    }
    /// Provider context that owned the finished execution.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Execution whose tools and review identities were released.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Reported terminal outcome, or the cause that forced failed execution cleanup.
    pub fn result(&self) -> &Result<ExecutionOutcome, PermissionCancellationReason> {
        &self.result
    }
}
