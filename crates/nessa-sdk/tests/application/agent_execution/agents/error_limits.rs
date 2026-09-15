use super::{MAX_BYTES, MAX_DEPTH, MAX_NODES};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::hooks::{HookError, HookFailure};
use crate::application::agent_execution::providers::{
    CleanupReport, CloseOutcome, ExecutionReport, ProviderOperationFailure, ProviderSessionState,
    ResourceCleanup,
};
use crate::application::agent_execution::sessions::StorageError;
use crate::domain::agent_execution::executions::ExecutionOutcome;

#[test]
fn oversized_diagnostics_are_bounded_before_clone_including_spare_capacity() {
    let mut spare = String::with_capacity(MAX_BYTES + 1);
    spare.push('x');
    for error in [
        AgentError::Configuration(spare),
        AgentError::Unsupported("x".repeat(MAX_BYTES)),
        AgentError::InvalidInput("x".repeat(MAX_BYTES)),
        AgentError::Protocol("x".repeat(MAX_BYTES)),
        AgentError::Transport("x".repeat(MAX_BYTES)),
        AgentError::Storage(StorageError::Io("x".repeat(MAX_BYTES))),
        AgentError::Storage(StorageError::Corrupt("x".repeat(MAX_BYTES))),
        AgentError::BeforeInvocationHook(HookFailure {
            index: 0,
            error: HookError::Failed("x".repeat(MAX_BYTES)),
        }),
    ] {
        assert!(error.validate_retained_size().is_err());
        let bounded = error.bounded();
        assert!(matches!(
            bounded,
            AgentError::DiagnosticLimit
                | AgentError::Configuration(_)
                | AgentError::Unsupported(_)
                | AgentError::InvalidInput(_)
                | AgentError::Protocol(_)
                | AgentError::Transport(_)
        ));
        assert!(bounded.validate_retained_size().is_ok());
        assert_eq!(bounded.clone(), bounded);
    }
}

#[test]
fn cumulative_hook_and_tree_budgets_apply_without_large_individual_strings() {
    let error = AgentError::AfterInvocationHooks {
        failures: (0..MAX_NODES)
            .map(|index| HookFailure {
                index,
                error: HookError::Panicked,
            })
            .collect(),
        execution_result: Box::new(Err(AgentError::Deadline)),
    };
    assert!(error.validate_retained_size().is_err());
    assert_eq!(error.bounded(), AgentError::DiagnosticLimit);
    let mut error = AgentError::Deadline;
    for _ in 0..MAX_DEPTH {
        error = AgentError::ExecutionObservation {
            error: Box::new(error),
            execution_result: None,
        };
    }
    assert!(error.validate_retained_size().is_err());
    assert_eq!(error.bounded(), AgentError::DiagnosticLimit);
}

#[test]
fn deep_rejected_error_is_classified_and_destroyed_without_recursion() {
    let mut error = AgentError::OperationAndCleanupFailure {
        operation_error: Box::new(AgentError::Deadline),
        cleanup_error: Box::new(AgentError::AuditAndCleanupFailure),
    };
    for _ in 0..50_000 {
        error = AgentError::ExecutionObservation {
            error: Box::new(error),
            execution_result: None,
        };
    }
    assert_eq!(error.bounded(), AgentError::DiagnosticLimit);
}

#[test]
fn diagnostic_normalization_preserves_explicit_resource_and_audit_reports() {
    for confirmed in [false, true] {
        let resource = if confirmed {
            ResourceCleanup::Confirmed(CloseOutcome { forced: false })
        } else {
            ResourceCleanup::Unconfirmed(AgentError::Protocol("still owned".into()))
        };
        let report = CleanupReport::new(
            resource.clone(),
            Err(AgentError::AfterInvocationHooks {
                failures: Vec::new(),
                execution_result: Box::new(Err(AgentError::Transport("x".repeat(MAX_BYTES)))),
            }),
        );
        assert_eq!(report.resources(), &resource);
        assert_eq!(report.is_confirmed(), confirmed);
        assert_eq!(report.audit(), &Err(AgentError::DiagnosticLimit));
        let failure = ProviderOperationFailure::new(
            AgentError::MultipleOperationFailures {
                first_error: Box::new(AgentError::Provider { code: 42 }),
                subsequent_error: Box::new(AgentError::Transport("x".repeat(MAX_BYTES))),
            },
            ProviderSessionState::CleanupReported(report.clone()),
        );
        assert_eq!(failure.error(), &AgentError::DiagnosticLimit);
        assert_eq!(
            failure.session_state(),
            &ProviderSessionState::CleanupReported(report)
        );
        assert_eq!(failure.clone(), failure);
    }
    let valid = AgentError::OperationAndCleanupFailure {
        operation_error: Box::new(AgentError::Deadline),
        cleanup_error: Box::new(AgentError::AuditFailure),
    };
    assert_eq!(valid.clone().bounded(), valid);
}

#[test]
fn diagnostic_summary_cannot_erase_separately_reported_provider_settlement() {
    for provider_result in [
        Ok(ExecutionOutcome::Completed),
        Err(AgentError::Provider { code: 42 }),
    ] {
        let provider_state = ProviderSessionState::CleanupRequired;
        let settlement = ExecutionReport::new(
            Some(provider_result.clone()),
            Some(AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(AgentError::Deadline),
                cleanup_error: Box::new(AgentError::Transport("x".repeat(MAX_BYTES))),
            }),
            provider_state.clone(),
        );
        assert_eq!(settlement.provider_result(), Some(&provider_result));
        assert_eq!(settlement.failure(), Some(&AgentError::DiagnosticLimit));
        assert_eq!(settlement.session_state(), &provider_state);
    }
}

#[test]
fn identical_diagnostics_do_not_determine_cleanup_authority() {
    let confirmed = CleanupReport::new(
        ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
        Err(AgentError::DiagnosticLimit),
    );
    let unconfirmed = CleanupReport::unconfirmed(AgentError::DiagnosticLimit);
    assert_eq!(
        confirmed.clone().into_result(),
        unconfirmed.clone().into_result()
    );
    assert!(confirmed.is_confirmed());
    assert!(!unconfirmed.is_confirmed());
}
