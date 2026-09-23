//! Combine operation, teardown audit, and process cleanup evidence without precedence loss.
use crate::application::agent_execution::{
    agents::AgentError, executions::ExecutionController, providers::SessionCloseRequest,
};
use crate::domain::agent_execution::permissions::PermissionCancellationReason;

const MAX_RETAINED_CATEGORY_FACTS: usize = 128;
const MAX_PROJECTED_CATEGORY_FACTS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AuditAttemptId(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OperationEffectId {
    pub(super) sequence: u64,
    pub(super) phase: OperationEffectPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OperationEffectPhase {
    Worker,
    PermissionDelivery,
    EventDelivery,
    Teardown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerminalFailureSource {
    Audit,
    Operation,
}

struct FailureFact<I> {
    id: I,
    error: AgentError,
}

/// Bounded diagnostic evidence for one worker generation's terminal settlement.
///
/// The two categories reserve independent budgets so saturation can never turn
/// an audit failure into operation evidence or hide an independent operation.
pub(super) struct SettlementFacts {
    audits: Vec<FailureFact<AuditAttemptId>>,
    operations: Vec<FailureFact<OperationEffectId>>,
    audit_overflow: bool,
    operation_overflow: bool,
}
impl SettlementFacts {
    pub(super) fn new() -> Self {
        Self {
            audits: Vec::new(),
            operations: Vec::new(),
            audit_overflow: false,
            operation_overflow: false,
        }
    }
    pub(super) fn record_audit(&mut self, id: AuditAttemptId) {
        if self.audits.iter().any(|fact| fact.id == id) || self.audit_overflow {
            return;
        }
        if self.audits.len() == MAX_RETAINED_CATEGORY_FACTS {
            self.audit_overflow = true;
            return;
        }
        self.audits.push(FailureFact {
            id,
            error: AgentError::AuditFailure,
        });
    }
    pub(super) fn record_operation(&mut self, id: OperationEffectId, error: AgentError) {
        if self.operations.iter().any(|fact| fact.id == id) || self.operation_overflow {
            return;
        }
        if self.operations.len() == MAX_RETAINED_CATEGORY_FACTS {
            self.operation_overflow = true;
            return;
        }
        self.operations.push(FailureFact {
            id,
            error: error.bounded(),
        });
    }
    pub(super) fn audit_result(&self) -> Result<(), AgentError> {
        Self::project(&self.audits, self.audit_overflow).map_or(Ok(()), Err)
    }
    pub(super) fn operation_failure(&self) -> Option<AgentError> {
        Self::project(&self.operations, self.operation_overflow)
    }
    fn project<I>(facts: &[FailureFact<I>], overflow: bool) -> Option<AgentError> {
        let retained = MAX_PROJECTED_CATEGORY_FACTS
            - usize::from(overflow || facts.len() >= MAX_PROJECTED_CATEGORY_FACTS);
        let mut errors = facts
            .iter()
            .take(retained)
            .map(|fact| fact.error.clone())
            .collect::<Vec<_>>();
        if overflow || facts.len() > retained {
            errors.push(AgentError::DiagnosticLimit);
        }
        Self::balanced(&errors).map(AgentError::bounded)
    }
    fn balanced(errors: &[AgentError]) -> Option<AgentError> {
        match errors {
            [] => None,
            [error] => Some(error.clone()),
            errors => {
                let middle = errors.len() / 2;
                Some(AgentError::MultipleOperationFailures {
                    first_error: Box::new(Self::balanced(&errors[..middle])?),
                    subsequent_error: Box::new(Self::balanced(&errors[middle..])?),
                })
            }
        }
    }
}

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
