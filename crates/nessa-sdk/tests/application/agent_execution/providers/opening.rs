//! Opening resource ownership is explicit and independent of diagnostic shape.
use super::*;

struct Cleanup;
impl ProviderCleanup for Cleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            let result: Result<CloseOutcome, AgentError> =
                { async { Err(AgentError::CleanupUncertain) } }.await;
            match result {
                Ok(outcome) => CleanupReport::confirmed(outcome),
                Err(error) => CleanupReport::unconfirmed(error),
            }
        })
    }
}

fn uncertain_summary() -> AgentError {
    AgentError::DiagnosticLimit
}

#[test]
fn explicit_opening_ownership_is_independent_of_direct_nested_and_summary_diagnostics() {
    for cause in [
        AgentError::CleanupUncertain,
        AgentError::AuditAndCleanupFailure,
        AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Deadline),
            cleanup_error: Box::new(AgentError::Transport("termination failed".into())),
        },
        AgentError::StorageInitialization {
            error: StorageError::Io("initial save failed".into()),
            cleanup_result: Box::new(Err(AgentError::CleanupUncertain)),
        },
        uncertain_summary(),
    ] {
        let detached = ProviderOpenError::no_resources(cause.clone());
        assert_eq!(detached.cause(), &cause);
        assert!(detached.cleanup().is_none());
        let error = ProviderOpenError::with_cleanup(cause.clone(), Arc::new(Cleanup));
        assert_eq!(error.cause(), &cause);
        assert!(error.cleanup().is_some());
    }
}

#[test]
fn resource_free_opening_preserves_confirmed_cleanup_and_pre_attachment_errors() {
    for cause in [
        AgentError::Configuration("invalid configuration".into()),
        AgentError::Unsupported("provider unavailable".into()),
        AgentError::Transport("spawn failed".into()),
        AgentError::AuditFailure,
        AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Deadline),
            cleanup_error: Box::new(AgentError::AuditFailure),
        },
    ] {
        let error = ProviderOpenError::no_resources(cause.clone());
        assert_eq!(error.cause(), &cause);
        assert!(error.cleanup().is_none());
    }
    // A non-cleanup cause may still own partially initialized resources.
    let error = ProviderOpenError::with_cleanup(AgentError::Deadline, Arc::new(Cleanup));
    assert_eq!(error.cause(), &AgentError::Deadline);
    assert!(error.cleanup().is_some());
}

#[test]
fn opening_error_bounds_deep_diagnostics_without_changing_resource_ownership() {
    let mut cause = AgentError::CleanupUncertain;
    for _ in 0..50_000 {
        cause = AgentError::MultipleOperationFailures {
            first_error: Box::new(AgentError::Closed),
            subsequent_error: Box::new(cause),
        };
    }
    let detached = ProviderOpenError::no_resources(cause);
    assert!(detached.cleanup().is_none());
    let rejected = detached.cause().clone();
    assert!(matches!(rejected, AgentError::DiagnosticLimit));
    let error = ProviderOpenError::with_cleanup(rejected, Arc::new(Cleanup));
    assert!(matches!(error.cause(), AgentError::DiagnosticLimit));
    assert!(error.cleanup().is_some());
}

#[test]
fn every_recursive_opening_error_branch_preserves_embedded_uncertainty() {
    let nested = || AgentError::PermissionAnswerDeliveryAndAuditFailure {
        delivery_error: Box::new(AgentError::CleanupUncertain),
        cleanup_error: None,
    };
    let storage = || StorageError::Io("storage failed".into());
    for cause in [
        nested(),
        AgentError::MultipleOperationFailures {
            first_error: Box::new(nested()),
            subsequent_error: Box::new(AgentError::Closed),
        },
        AgentError::MultipleOperationFailures {
            first_error: Box::new(AgentError::Closed),
            subsequent_error: Box::new(nested()),
        },
        AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(nested()),
            cleanup_error: Box::new(AgentError::AuditFailure),
        },
        AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Closed),
            cleanup_error: Box::new(nested()),
        },
        AgentError::ExecutionObservation {
            error: Box::new(nested()),
            execution_result: None,
        },
        AgentError::ExecutionObservation {
            error: Box::new(AgentError::Closed),
            execution_result: Some(Box::new(Err(nested()))),
        },
        AgentError::StorageDuringClose {
            error: storage(),
            cleanup_result: Box::new(Err(nested())),
        },
        AgentError::StorageInitialization {
            error: storage(),
            cleanup_result: Box::new(Err(nested())),
        },
        AgentError::StorageAfterExecution {
            error: storage(),
            execution_result: Box::new(Err(nested())),
        },
        AgentError::AfterInvocationHooks {
            failures: Vec::new(),
            execution_result: Box::new(Err(nested())),
        },
        AgentError::PermissionAnswerDeliveryAndAuditFailure {
            delivery_error: Box::new(AgentError::Deadline),
            cleanup_error: Some(Box::new(nested())),
        },
    ] {
        let detached = ProviderOpenError::no_resources(cause.clone());
        assert_eq!(detached.cause(), &cause);
        assert!(detached.cleanup().is_none());
        let owned = ProviderOpenError::with_cleanup(cause.clone(), Arc::new(Cleanup));
        assert_eq!(owned.cause(), &cause);
        assert!(owned.cleanup().is_some());
    }
    // Summarization preserves diagnostic facts without creating or releasing resources.
    let oversized = AgentError::PermissionAnswerDeliveryAndAuditFailure {
        delivery_error: Box::new(AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Transport("x".repeat(1024 * 1024))),
            cleanup_error: Box::new(AgentError::CleanupUncertain),
        }),
        cleanup_error: None,
    };
    assert!(matches!(
        ProviderOpenError::no_resources(oversized).cause(),
        AgentError::DiagnosticLimit
    ));
    // Ordinary delivery failure with confirmed cleanup remains an admissible resource-free error.
    let confirmed = AgentError::PermissionAnswerDeliveryAndAuditFailure {
        delivery_error: Box::new(AgentError::Deadline),
        cleanup_error: Some(Box::new(AgentError::AuditFailure)),
    };
    assert!(ProviderOpenError::no_resources(confirmed)
        .cleanup()
        .is_none());
}
