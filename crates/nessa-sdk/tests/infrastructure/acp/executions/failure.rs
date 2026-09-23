//! Explicit cleanup evidence stays authoritative across diagnostic combinations.
use super::*;
use crate::application::agent_execution::providers::{
    CleanupReport, CloseOutcome, ExecutionReport, FinalizedExecutionProjection,
    FinalizedFailureComponent, ProviderIdentity, ProviderSessionState, ResourceCleanup,
};
use crate::application::agent_execution::{
    executions::{ExecutionRequest, SubmissionMode},
    permissions::ActionContext,
    sessions::storage::{
        InvocationCancellationEvent, InvocationRecord, ProviderContext, SessionSnapshot,
        SessionStorage, SubmissionAcknowledgement,
    },
};
use crate::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, SchedulingCause},
    prompts::{PromptText, UserMessage},
    sessions::{ExecutionSessionId, SessionId},
};
use crate::infrastructure::session_storage::InMemoryStorage;

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
    facts.record_audit(AuditEffectPhase::Lifecycle);
    facts.record_audit(AuditEffectPhase::Lifecycle);
    let error = AgentError::AuditFailure;
    facts.record_operation(OperationEffectPhase::Worker, error.clone());
    facts.record_operation(OperationEffectPhase::Worker, error.clone());
    let projection = facts.finalize();
    assert_eq!(
        projection.components(),
        &[
            FinalizedFailureComponent::Audit,
            FinalizedFailureComponent::Audit,
            FinalizedFailureComponent::Operation(error.clone()),
            FinalizedFailureComponent::Operation(error),
        ]
    );
}

#[test]
fn only_exact_permission_delivery_correlation_combines_operation_and_audit() {
    let mut facts = SettlementFacts::new();
    let correlated = EffectCorrelation(7);
    facts.record_operation(
        OperationEffectPhase::PermissionDelivery(correlated),
        AgentError::Deadline,
    );
    facts.record_audit(AuditEffectPhase::PermissionDelivery(correlated));
    facts.record_audit(AuditEffectPhase::Lifecycle);
    assert_eq!(
        facts.finalize().components(),
        &[
            FinalizedFailureComponent::PermissionDeliveryAndAudit {
                delivery_error: AgentError::Deadline,
            },
            FinalizedFailureComponent::Audit,
        ]
    );
}

#[test]
fn finalized_sources_preserve_provider_and_local_results_with_late_failures() {
    let provider = ExecutionReport::finalized_provider(
        Some(Err(AgentError::Deadline)),
        ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
        None,
        FinalizedExecutionProjection::new(vec![FinalizedFailureComponent::Audit]).unwrap(),
    );
    let AgentError::ExecutionObservation {
        execution_result: Some(provider_result),
        ..
    } = provider.into_result().unwrap_err()
    else {
        panic!("provider result must remain distinct from the later audit failure")
    };
    assert_eq!(*provider_result, Err(AgentError::Deadline));

    let local = ExecutionReport::finalized_local_cancellation(
        ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
        None,
        FinalizedExecutionProjection::new(vec![FinalizedFailureComponent::Operation(
            AgentError::Transport("late failure".into()),
        )])
        .unwrap(),
    );
    let AgentError::ExecutionObservation {
        execution_result: Some(local_result),
        ..
    } = local.into_result().unwrap_err()
    else {
        panic!("local cancellation must retain its source beside a late failure")
    };
    assert_eq!(*local_result, Ok(ExecutionOutcome::Cancelled));
}

#[test]
fn provider_result_coverage_does_not_remint_the_same_failure_as_an_operation() {
    let mut facts = SettlementFacts::new();
    let cursor = facts.cursor();
    let provider_fact = facts.record_provider_result();
    let coverage = facts.coverage_since(cursor);
    assert_eq!(coverage, vec![provider_fact]);
    assert!(!facts.coverage_has_operation(&coverage));
    assert!(facts.finalize().components().is_empty());
}

#[test]
fn finalized_recipe_normalization_is_idempotent_and_rejects_malformed_overflow_order() {
    let oversized = AgentError::Transport("x".repeat(2 * 1024 * 1024));
    let once =
        FinalizedExecutionProjection::new(vec![FinalizedFailureComponent::Operation(oversized)])
            .unwrap();
    let twice = FinalizedExecutionProjection::new(once.components().to_vec()).unwrap();
    assert_eq!(once, twice);
    assert!(FinalizedExecutionProjection::new(vec![
        FinalizedFailureComponent::OperationOverflow,
        FinalizedFailureComponent::Operation(AgentError::Deadline),
    ])
    .is_err());
    assert!(FinalizedExecutionProjection::new(vec![
        FinalizedFailureComponent::AuditOverflow,
        FinalizedFailureComponent::AuditOverflow,
    ])
    .is_err());
}

#[tokio::test]
async fn finalized_provider_and_local_recipes_survive_storage_reload_exactly() {
    for local in [false, true] {
        let projection = FinalizedExecutionProjection::new(vec![
            FinalizedFailureComponent::Operation(AgentError::Deadline),
            FinalizedFailureComponent::Audit,
        ])
        .unwrap();
        let report = if local {
            ExecutionReport::finalized_local_cancellation(
                ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                None,
                projection,
            )
        } else {
            ExecutionReport::finalized_provider(
                Some(Ok(ExecutionOutcome::Completed)),
                ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                None,
                projection,
            )
        };
        let session_id = SessionId::new(if local {
            "finalized-local"
        } else {
            "finalized-provider"
        })
        .unwrap();
        let execution_id = ExecutionId::new("execution").unwrap();
        let actor = ActionContext::new("user", "test", "invoke").unwrap();
        let snapshot = SessionSnapshot {
            queue_history: Vec::new(),
            id: session_id.clone(),
            provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
            provider_context: ProviderContext::Recorded(
                ExecutionSessionId::new("provider-context").unwrap(),
            ),
            invocations: vec![InvocationRecord {
                target_event_offset: None,
                submission: SubmissionMode::Immediate,
                request: ExecutionRequest {
                    execution_id,
                    user_message: UserMessage::text_only(PromptText::new("input").unwrap()),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 1,
                },
                actor: actor.clone(),
                acknowledgement: SubmissionAcknowledgement::Acknowledged,
                events: Vec::new(),
                scheduling: Vec::new(),
                cancellation: None,
                provider_report: Some(report.clone()),
                local_cancellation: local.then_some(InvocationCancellationEvent {
                    cause: SchedulingCause::SessionClosed,
                    actor: Some(actor),
                }),
                local_outcome: None,
                result: Some(report.clone().into_result()),
            }],
        };
        let storage = InMemoryStorage::new();
        let lease = storage.open(session_id).await.unwrap();
        lease.save(snapshot).await.unwrap();
        let restored = lease.load().await.unwrap().unwrap();
        assert_eq!(restored.invocations[0].provider_report, Some(report));
    }
}

#[test]
fn settlement_fact_budgets_preserve_both_categories_in_either_arrival_order() {
    for audit_first in [false, true] {
        let mut facts = SettlementFacts::new();
        if audit_first {
            for _ in 0..=MAX_RETAINED_CATEGORY_FACTS {
                facts.record_audit(AuditEffectPhase::Lifecycle);
            }
            for _ in 0..=MAX_RETAINED_CATEGORY_FACTS {
                facts.record_operation(OperationEffectPhase::Worker, AgentError::Deadline);
            }
        } else {
            for _ in 0..=MAX_RETAINED_CATEGORY_FACTS {
                facts.record_operation(OperationEffectPhase::Worker, AgentError::Deadline);
            }
            for _ in 0..=MAX_RETAINED_CATEGORY_FACTS {
                facts.record_audit(AuditEffectPhase::Lifecycle);
            }
        }
        let projection = facts.finalize();
        assert_eq!(
            projection
                .components()
                .iter()
                .filter(|component| matches!(component, FinalizedFailureComponent::Audit))
                .count(),
            MAX_RETAINED_CATEGORY_FACTS
        );
        assert_eq!(
            projection
                .components()
                .iter()
                .filter(|component| matches!(component, FinalizedFailureComponent::Operation(_)))
                .count(),
            MAX_RETAINED_CATEGORY_FACTS
        );
        assert!(projection
            .components()
            .contains(&FinalizedFailureComponent::AuditOverflow));
        assert!(projection
            .components()
            .contains(&FinalizedFailureComponent::OperationOverflow));
        let report = ExecutionReport::finalized_provider(
            None,
            ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
            None,
            projection,
        );
        let ProviderSessionState::CleanupReported(cleanup) = report.session_state() else {
            panic!("finalized settlement must expose cleanup projections")
        };
        let audit = cleanup.audit().clone().unwrap_err();
        let operation = cleanup.operation_failure().cloned().unwrap();
        assert!(contains_diagnostic_limit(&audit));
        assert!(contains_diagnostic_limit(&operation));
        assert_eq!(count_matching(&audit, &AgentError::AuditFailure), 31);
        assert_eq!(count_matching(&operation, &AgentError::Deadline), 31);
        audit.validate_retained_size().unwrap();
        operation.validate_retained_size().unwrap();
        let combined = report.into_result().unwrap_err();
        assert_eq!(count_matching(&combined, &AgentError::Deadline), 31);
        assert_eq!(count_matching(&combined, &AgentError::AuditFailure), 31);
        combined.validate_retained_size().unwrap();
    }
}
