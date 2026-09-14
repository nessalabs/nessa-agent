//! Evidence produced by the live aggregate's permanent open-to-closed transition.
#![deny(missing_docs)]

use crate::domain::agent_execution::{
    executions::ExecutionId,
    permissions::{PermissionCancellationReason, PermissionCancellationReasonView},
    sessions::ExecutionSessionId,
    ExecutionError,
};

/// Immutable evidence that one live attachment changed from open to closed.
/// The application attaches the known initiator and delivers this through its audit port.
/// It records local closure, not confirmation of provider process termination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionClosure {
    session_id: ExecutionSessionId,
    execution_id: Option<ExecutionId>,
    reason: PermissionCancellationReason,
}
impl SessionClosure {
    pub(crate) fn new(
        session_id: ExecutionSessionId,
        execution_id: Option<ExecutionId>,
        reason: PermissionCancellationReason,
    ) -> Result<Self, ExecutionError> {
        Self::validate_reason(&reason)?;
        if reason == PermissionCancellationReason::execution_failed() && execution_id.is_none() {
            return Err(ExecutionError::InvalidSessionClosureReason);
        }
        Ok(Self {
            session_id,
            execution_id,
            reason,
        })
    }
    pub(crate) fn validate_reason(
        reason: &PermissionCancellationReason,
    ) -> Result<(), ExecutionError> {
        match reason.view() {
            PermissionCancellationReasonView::ProviderWithdrawal
            | PermissionCancellationReasonView::ExecutionFinished
            | PermissionCancellationReasonView::Custom(_) => {
                Err(ExecutionError::InvalidSessionClosureReason)
            }
            PermissionCancellationReasonView::SessionClosed
            | PermissionCancellationReasonView::SessionFailed
            | PermissionCancellationReasonView::ExecutionFailed
            | PermissionCancellationReasonView::DeadlineExceeded
            | PermissionCancellationReasonView::EventConsumerDropped
            | PermissionCancellationReasonView::SessionHandlesDropped => Ok(()),
        }
    }
    /// Provider context whose live attachment moved from open to closed.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Execution active at closure, absent when the attachment was idle.
    /// ExecutionFailed always retains an execution; idle failure uses SessionFailed.
    pub fn execution_id(&self) -> Option<&ExecutionId> {
        self.execution_id.as_ref()
    }
    /// Explicit or automatic cause of this transition, retained exactly.
    pub fn reason(&self) -> &PermissionCancellationReason {
        &self.reason
    }
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/session_closure.rs"]
mod tests;
