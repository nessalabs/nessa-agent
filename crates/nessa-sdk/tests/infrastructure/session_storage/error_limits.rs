//! Restored diagnostics are bounded before retention and provider attachment.
use super::custom_storage::assert_custom_retention_admission;
use super::*;

#[tokio::test]
async fn oversized_and_deep_errors_cannot_enter_storage_or_custom_restoration() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    let mut deep = AgentError::Deadline;
    for _ in 0..40 {
        deep = AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(deep),
            cleanup_error: Box::new(AgentError::AuditFailure),
        };
    }
    for error in [AgentError::Transport("x".repeat(1024 * 1024)), deep] {
        let mut value = snapshot("error-limits");
        value.invocations[0].result = Some(Err(error));
        for storage in &stores {
            let lease = storage.open(value.id.clone()).await.unwrap();
            let original = snapshot("error-limits");
            lease.save(original.clone()).await.unwrap();
            assert!(matches!(
                lease.save(value.clone()).await,
                Err(StorageError::Corrupt(_))
            ));
            assert_same(&lease.load().await.unwrap().unwrap(), &original);
        }
        assert_custom_retention_admission(value, false).await;
    }
}

#[tokio::test]
async fn acknowledgement_errors_are_bounded_before_custom_storage_clone() {
    fn acknowledgement(case: usize) -> SubmissionAcknowledgement {
        match case {
            0 => SubmissionAcknowledgement::Failed {
                audit: Some(AgentError::Protocol("x".repeat(1024 * 1024))),
                storage: None,
            },
            1 => {
                let mut deep = AgentError::Deadline;
                for _ in 0..40 {
                    deep = AgentError::OperationAndCleanupFailure {
                        operation_error: Box::new(deep),
                        cleanup_error: Box::new(AgentError::AuditFailure),
                    };
                }
                SubmissionAcknowledgement::Failed {
                    audit: Some(deep),
                    storage: None,
                }
            }
            2 => {
                let mut spare = String::with_capacity(1024 * 1024);
                spare.push_str("small diagnostic");
                SubmissionAcknowledgement::Failed {
                    audit: None,
                    storage: Some(StorageError::Io(spare)),
                }
            }
            _ => unreachable!("three acknowledgement limit cases"),
        }
    }

    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for case in 0..3 {
        for storage in &stores {
            let mut value = snapshot("acknowledgement-error-limits");
            value.invocations[0].acknowledgement = acknowledgement(case);
            let lease = storage.open(value.id.clone()).await.unwrap();
            assert!(matches!(
                lease.save(value).await,
                Err(StorageError::Corrupt(_))
            ));
        }
        let mut value = snapshot("acknowledgement-error-limits");
        value.invocations[0].acknowledgement = acknowledgement(case);
        assert_custom_retention_admission(value, false).await;
    }
}

#[tokio::test]
async fn bounded_diagnostic_summary_round_trips_without_claiming_missing_details() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let lease = storage.open(id("error-summary")).await.unwrap();
    let mut value = snapshot("error-summary");
    value.invocations[0].result = Some(Err(AgentError::DiagnosticLimit));
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
}

#[tokio::test]
async fn rejected_save_disposes_deep_error_even_when_its_future_is_never_polled() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let lease = storage.open(id("deep")).await.unwrap();
        for matching_identity in [false, true] {
            let mut value = snapshot(if matching_identity { "deep" } else { "wrong" });
            let mut error = AgentError::Deadline;
            for _ in 0..50_000 {
                error = AgentError::ExecutionObservation {
                    error: Box::new(error),
                    execution_result: None,
                };
            }
            value.invocations[0].result = Some(Err(error));
            // Validation/disposal must precede construction of the returned future.
            drop(lease.save(value));
            assert!(lease.load().await.unwrap().is_none());
        }
    }
}
