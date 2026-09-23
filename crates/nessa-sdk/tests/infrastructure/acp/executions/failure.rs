//! Explicit cleanup evidence stays authoritative across diagnostic combinations.
use super::*;
use crate::application::agent_execution::providers::{
    CleanupReport, CloseOutcome, ExecutionReport, ProviderSessionState, ResourceCleanup,
};
use crate::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome},
    sessions::ExecutionSessionId,
};

#[test]
fn cleanup_confirmation_is_independent_of_every_primary_and_audit_error() {
    for primary in [
        AgentError::Deadline,
        AgentError::CleanupUncertain,
        AgentError::Provider {
            code: -42,
            diagnostic: None,
        },
        AgentError::AuditFailure,
    ] {
        for confirmed in [false, true] {
            for audit_failed in [false, true] {
                let resources = if confirmed {
                    ResourceCleanup::Confirmed(CloseOutcome { forced: true })
                } else {
                    ResourceCleanup::Unconfirmed(AgentError::Deadline)
                };
                let report = CleanupReport::new(
                    resources.clone(),
                    if audit_failed {
                        Err(AgentError::AuditFailure)
                    } else {
                        Ok(())
                    },
                );
                let settlement = ExecutionReport::new(
                    Some(Err(primary.clone())),
                    None,
                    ProviderSessionState::CleanupReported(report.clone()),
                );
                assert_eq!(report.is_confirmed(), confirmed);
                assert_eq!(report.resources(), &resources);
                assert_eq!(settlement.provider_result(), Some(&Err(primary.clone())));
                assert!(settlement.into_result().is_err());
            }
        }
    }
}

#[test]
fn successful_physical_retry_preserves_failed_audit_and_original_execution() {
    let report = CleanupReport::new(
        ResourceCleanup::Unconfirmed(AgentError::Deadline),
        Err(AgentError::AuditFailure),
    );
    let recovered =
        report.with_resources(ResourceCleanup::Confirmed(CloseOutcome { forced: true }));
    assert!(recovered.is_confirmed());
    assert_eq!(recovered.audit(), &Err(AgentError::AuditFailure));
    let settled = ExecutionReport::new(
        Some(Ok(ExecutionOutcome::Completed)),
        None,
        ProviderSessionState::CleanupReported(recovered),
    );
    let AgentError::ExecutionObservation {
        execution_result: Some(result),
        ..
    } = settled.into_result().unwrap_err()
    else {
        panic!("known result must survive failed audit")
    };
    assert_eq!(*result, Ok(ExecutionOutcome::Completed));
}

#[test]
fn independent_operation_failures_preserve_order_without_classifying_resources() {
    let first = AgentError::AuditFailure;
    let subsequent = AgentError::Deadline;
    assert_eq!(
        retain_admitted_failure(Some(first.clone()), subsequent.clone()),
        AgentError::MultipleOperationFailures {
            first_error: Box::new(first),
            subsequent_error: Box::new(subsequent)
        }
    );
}

#[test]
fn unexpected_finish_failure_is_reported_unless_execution_was_already_released() {
    for active in [false, true] {
        for previous in [
            None,
            Some(AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(AgentError::Provider {
                    code: -42,
                    diagnostic: None,
                }),
                cleanup_error: Box::new(AgentError::AuditAndCleanupFailure),
            }),
        ] {
            let mut execution =
                ExecutionController::new(ExecutionSessionId::new("finish-check").unwrap());
            execution
                .begin_execution(ExecutionId::new("run").unwrap())
                .unwrap();
            if !active {
                let _records = execution
                    .finish_execution(
                        &ExecutionId::new("run").unwrap(),
                        Ok(ExecutionOutcome::Completed),
                    )
                    .unwrap();
            }
            let error = execution
                .finish_execution(
                    &ExecutionId::new("run").unwrap(),
                    Err(PermissionCancellationReason::execution_finished()),
                )
                .unwrap_err();
            assert_eq!(execution.active_execution_id().is_some(), active);
            let observed = execution_finish_failure(&execution, previous.clone(), error.clone());
            let expected = if !active && previous.is_some() {
                None
            } else {
                Some(match previous {
                    Some(first_error) => AgentError::MultipleOperationFailures {
                        first_error: Box::new(first_error),
                        subsequent_error: Box::new(error),
                    },
                    None => error,
                })
            };
            assert_eq!(observed, expected);
        }
    }
}

fn contains_diagnostic_limit(error: &AgentError) -> bool {
    match error {
        AgentError::DiagnosticLimit => true,
        AgentError::MultipleOperationFailures {
            first_error,
            subsequent_error,
        }
        | AgentError::OperationAndCleanupFailure {
            operation_error: first_error,
            cleanup_error: subsequent_error,
        } => contains_diagnostic_limit(first_error) || contains_diagnostic_limit(subsequent_error),
        _ => false,
    }
}

fn count_matching(error: &AgentError, expected: &AgentError) -> usize {
    match error {
        AgentError::MultipleOperationFailures {
            first_error,
            subsequent_error,
        }
        | AgentError::OperationAndCleanupFailure {
            operation_error: first_error,
            cleanup_error: subsequent_error,
        } => count_matching(first_error, expected) + count_matching(subsequent_error, expected),
        error if error == expected => 1,
        _ => 0,
    }
}

#[test]
fn settlement_fact_identity_preserves_equal_independent_failures() {
    let mut facts = SettlementFacts::new();
    facts.record_audit(AuditAttemptId(1));
    facts.record_audit(AuditAttemptId(1));
    assert_eq!(facts.audit_result(), Err(AgentError::AuditFailure));
    facts.record_audit(AuditAttemptId(2));
    assert_eq!(
        facts.audit_result(),
        Err(AgentError::MultipleOperationFailures {
            first_error: Box::new(AgentError::AuditFailure),
            subsequent_error: Box::new(AgentError::AuditFailure),
        })
    );

    let error = AgentError::AuditFailure;
    facts.record_operation(
        OperationEffectId {
            sequence: 1,
            phase: OperationEffectPhase::Worker,
        },
        error.clone(),
    );
    facts.record_operation(
        OperationEffectId {
            sequence: 2,
            phase: OperationEffectPhase::Worker,
        },
        error.clone(),
    );
    assert_eq!(
        facts.operation_failure(),
        Some(AgentError::MultipleOperationFailures {
            first_error: Box::new(error.clone()),
            subsequent_error: Box::new(error),
        })
    );
}

#[test]
fn settlement_fact_budgets_preserve_both_categories_in_either_arrival_order() {
    for audit_first in [false, true] {
        let mut facts = SettlementFacts::new();
        if audit_first {
            for index in 0..=MAX_RETAINED_CATEGORY_FACTS as u64 {
                facts.record_audit(AuditAttemptId(index));
            }
            for index in 0..=MAX_RETAINED_CATEGORY_FACTS as u64 {
                facts.record_operation(
                    OperationEffectId {
                        sequence: index,
                        phase: OperationEffectPhase::PermissionDelivery,
                    },
                    AgentError::Deadline,
                );
            }
        } else {
            for index in 0..=MAX_RETAINED_CATEGORY_FACTS as u64 {
                facts.record_operation(
                    OperationEffectId {
                        sequence: index,
                        phase: OperationEffectPhase::PermissionDelivery,
                    },
                    AgentError::Deadline,
                );
            }
            for index in 0..=MAX_RETAINED_CATEGORY_FACTS as u64 {
                facts.record_audit(AuditAttemptId(index));
            }
        }
        let audit = facts.audit_result().unwrap_err();
        let operation = facts.operation_failure().unwrap();
        assert!(contains_diagnostic_limit(&audit));
        assert!(contains_diagnostic_limit(&operation));
        assert_eq!(count_matching(&audit, &AgentError::AuditFailure), 31);
        assert_eq!(count_matching(&operation, &AgentError::Deadline), 31);
        audit.validate_retained_size().unwrap();
        operation.validate_retained_size().unwrap();
        let combined = CleanupReport::new(
            ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
            Err(audit),
        )
        .with_operation_failure(Some(operation))
        .into_result()
        .unwrap_err();
        let AgentError::OperationAndCleanupFailure {
            operation_error,
            cleanup_error,
        } = &combined
        else {
            panic!("both projected categories must remain independently visible")
        };
        assert_eq!(count_matching(operation_error, &AgentError::Deadline), 31);
        assert_eq!(count_matching(cleanup_error, &AgentError::AuditFailure), 31);
        combined.validate_retained_size().unwrap();
    }
}
