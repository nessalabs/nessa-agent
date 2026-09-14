//! Combine operation, teardown audit, and process cleanup evidence without precedence loss.
use crate::application::agent_execution::{
    agents::AgentError, executions::ExecutionController, providers::SessionCloseRequest,
};
use crate::domain::agent_execution::permissions::PermissionCancellationReason;

/// A delayed execution-failure shutdown may arrive after authoritative settlement.
/// It still retires the attachment, without inventing another failed execution.
pub(super) fn requested_close_reason(
    request: &SessionCloseRequest,
    has_active_execution: bool,
) -> PermissionCancellationReason {
    if matches!(request, SessionCloseRequest::ExecutionFailed) && !has_active_execution {
        PermissionCancellationReason::session_failed()
    } else {
        request.reason()
    }
}

/// Retain failures from separately admitted permission operations during teardown.
pub(super) fn retain_admitted_failure(
    previous: Option<AgentError>,
    next: AgentError,
) -> AgentError {
    match previous {
        Some(first_error) if first_error == next => first_error,
        Some(first_error) => AgentError::MultipleOperationFailures {
            first_error: Box::new(first_error),
            subsequent_error: Box::new(next),
        },
        None => next,
    }
}

/// An earlier terminal delivery failure can leave only the worker's reply active.
/// Ignore the repeated finish only when the controller already released execution
/// and a primary failure is retained. Every failure with active state is evidence.
pub(super) fn execution_finish_failure(
    execution: &ExecutionController,
    previous: Option<AgentError>,
    error: AgentError,
) -> Option<AgentError> {
    if execution.active_execution_id().is_none() && previous.is_some() {
        return None;
    }
    Some(match previous {
        Some(first_error) => AgentError::MultipleOperationFailures {
            first_error: Box::new(first_error),
            subsequent_error: Box::new(error),
        },
        None => error,
    })
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/executions/failure.rs"]
mod tests;
