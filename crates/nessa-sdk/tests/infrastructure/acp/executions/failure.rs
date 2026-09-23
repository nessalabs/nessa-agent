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
