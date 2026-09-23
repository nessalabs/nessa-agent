//! Combine operation, teardown audit, and process cleanup evidence without precedence loss.
use crate::application::agent_execution::{
    agents::AgentError,
    executions::ExecutionController,
    providers::{FinalizedExecutionProjection, FinalizedFailureComponent, SessionCloseRequest},
};
use crate::domain::agent_execution::permissions::PermissionCancellationReason;

pub(super) const MAX_RETAINED_CATEGORY_FACTS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct FactId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectCorrelation(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OperationEffectPhase {
    Worker,
    PermissionDelivery(EffectCorrelation),
    EventDelivery,
    Teardown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuditEffectPhase {
    Lifecycle,
    Cancellation,
    PermissionSelection,
    PermissionDelivery(EffectCorrelation),
}

enum SettlementFact {
    ProviderResult {
        id: FactId,
    },
    Audit {
        id: FactId,
        phase: AuditEffectPhase,
    },
    Operation {
        id: FactId,
        phase: OperationEffectPhase,
        error: AgentError,
    },
    AuditOverflow {
        id: FactId,
    },
    OperationOverflow {
        id: FactId,
    },
}

/// Bounded diagnostic evidence for one worker generation's terminal settlement.
///
/// The two categories reserve independent budgets so saturation can never turn
/// an audit failure into operation evidence or hide an independent operation.
pub(super) struct SettlementFacts {
    facts: Vec<SettlementFact>,
    next_id: u64,
    audit_count: usize,
    operation_count: usize,
    audit_overflow: Option<FactId>,
    operation_overflow: Option<FactId>,
}
impl SettlementFacts {
    pub(super) fn new() -> Self {
        Self {
            facts: Vec::new(),
            next_id: 0,
            audit_count: 0,
            operation_count: 0,
            audit_overflow: None,
            operation_overflow: None,
        }
    }
    fn next_id(&mut self) -> FactId {
        let id = FactId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("one worker cannot exhaust failure fact identities");
        id
    }
    pub(super) fn record_audit(&mut self, phase: AuditEffectPhase) -> Option<FactId> {
        if let Some(id) = self.audit_overflow {
            return Some(id);
        }
        if self.audit_count == MAX_RETAINED_CATEGORY_FACTS {
            let id = self.next_id();
            self.audit_overflow = Some(id);
            self.facts.push(SettlementFact::AuditOverflow { id });
            return Some(id);
        }
        let id = self.next_id();
        self.audit_count += 1;
        self.facts.push(SettlementFact::Audit { id, phase });
        Some(id)
    }
    pub(super) fn record_provider_result(&mut self) -> FactId {
        let id = self.next_id();
        self.facts.push(SettlementFact::ProviderResult { id });
        id
    }
    pub(super) fn record_operation(
        &mut self,
        phase: OperationEffectPhase,
        error: AgentError,
    ) -> Option<FactId> {
        if let Some(id) = self.operation_overflow {
            return Some(id);
        }
        if self.operation_count == MAX_RETAINED_CATEGORY_FACTS {
            let id = self.next_id();
            self.operation_overflow = Some(id);
            self.facts.push(SettlementFact::OperationOverflow { id });
            return Some(id);
        }
        let id = self.next_id();
        self.operation_count += 1;
        self.facts.push(SettlementFact::Operation {
            id,
            phase,
            error: error.bounded(),
        });
        Some(id)
    }
    pub(super) fn contains(&self, id: FactId) -> bool {
        self.facts.iter().any(|fact| match fact {
            SettlementFact::ProviderResult { id: fact_id }
            | SettlementFact::Audit { id: fact_id, .. }
            | SettlementFact::Operation { id: fact_id, .. } => *fact_id == id,
            SettlementFact::AuditOverflow { id: fact_id }
            | SettlementFact::OperationOverflow { id: fact_id } => *fact_id == id,
        })
    }
    pub(super) fn coverage_has_operation(&self, coverage: &[FactId]) -> bool {
        self.facts.iter().any(|fact| match fact {
            SettlementFact::Operation { id, .. } | SettlementFact::OperationOverflow { id } => {
                coverage.contains(id)
            }
            _ => false,
        })
    }
    pub(super) fn finalize(self) -> FinalizedExecutionProjection {
        let paired = self
            .facts
            .iter()
            .filter_map(|fact| match fact {
                SettlementFact::Operation {
                    phase: OperationEffectPhase::PermissionDelivery(correlation),
                    ..
                } => Some(*correlation),
                _ => None,
            })
            .filter(|correlation| {
                self.facts.iter().any(|fact| {
                    matches!(
                        fact,
                        SettlementFact::Audit {
                            phase: AuditEffectPhase::PermissionDelivery(candidate),
                            ..
                        } if candidate == correlation
                    )
                })
            })
            .collect::<Vec<_>>();
        let components = self
            .facts
            .into_iter()
            .filter_map(|fact| match fact {
                SettlementFact::Operation {
                    phase: OperationEffectPhase::PermissionDelivery(correlation),
                    error,
                    ..
                } if paired.contains(&correlation) => {
                    Some(FinalizedFailureComponent::PermissionDeliveryAndAudit {
                        delivery_error: error,
                    })
                }
                SettlementFact::Operation { error, .. } => {
                    Some(FinalizedFailureComponent::Operation(error))
                }
                SettlementFact::ProviderResult { .. } => None,
                SettlementFact::Audit {
                    phase: AuditEffectPhase::PermissionDelivery(correlation),
                    ..
                } if paired.contains(&correlation) => None,
                SettlementFact::Audit { .. } => Some(FinalizedFailureComponent::Audit),
                SettlementFact::AuditOverflow { .. } => {
                    Some(FinalizedFailureComponent::AuditOverflow)
                }
                SettlementFact::OperationOverflow { .. } => {
                    Some(FinalizedFailureComponent::OperationOverflow)
                }
            })
            .collect();
        FinalizedExecutionProjection::new(components)
            .expect("bounded worker facts always form a valid finalized projection")
    }
}

/// A terminal error and the exact already-recorded facts it represents.
pub(super) struct WorkerFailure {
    error: AgentError,
    coverage: Vec<FactId>,
}
impl WorkerFailure {
    pub(super) fn new(error: AgentError, coverage: impl IntoIterator<Item = FactId>) -> Self {
        Self {
            error,
            coverage: coverage.into_iter().collect(),
        }
    }
    pub(super) fn into_error(self) -> AgentError {
        self.error
    }
    pub(super) fn error(&self) -> &AgentError {
        &self.error
    }
    pub(super) fn with_error(self, error: AgentError) -> Self {
        Self {
            error,
            coverage: self.coverage,
        }
    }
    pub(super) fn validate(&self, facts: &SettlementFacts) {
        assert!(self.coverage.iter().all(|id| facts.contains(*id)));
    }
    pub(super) fn coverage(&self) -> &[FactId] {
        &self.coverage
    }
    pub(super) fn combine(self, subsequent: Self) -> Self {
        let mut coverage = self.coverage;
        coverage.extend(subsequent.coverage);
        Self {
            error: retain_admitted_failure(Some(self.error), subsequent.error),
            coverage,
        }
    }
}
impl From<AgentError> for WorkerFailure {
    fn from(error: AgentError) -> Self {
        Self {
            error,
            coverage: Vec::new(),
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
