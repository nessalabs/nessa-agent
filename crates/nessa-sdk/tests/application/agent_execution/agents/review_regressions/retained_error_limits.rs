//! Oversized provider failures are normalized before runtime evidence retention.
use super::*;

#[tokio::test]
async fn oversized_provider_errors_are_bounded_for_direct_and_queued_receipts() {
    for queued in [false, true] {
        let (agent, backend, storage) = probe(false).await;
        *backend.execution_attachment.lock().unwrap() =
            ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                forced: false,
            }));
        *backend.execution_error.lock().unwrap() = Some(AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Provider {
                code: 401,
                diagnostic: None,
            }),
            cleanup_error: Box::new(AgentError::StorageDuringClose {
                error: StorageError::Io("x".repeat(1024 * 1024)),
                cleanup_result: Box::new(Ok(CloseOutcome { forced: false })),
            }),
        });
        let result = if queued {
            agent
                .enqueue(input("oversized"), close_action())
                .await
                .unwrap()
                .wait()
                .await
        } else {
            agent.invoke(input("oversized"), close_action()).await
        };
        assert_eq!(
            result,
            Err(expected_terminal_error(AgentError::DiagnosticLimit))
        );
        assert_eq!(storage.snapshot().invocations[0].result, Some(result));
        agent.close(close_action()).await.unwrap();
    }
}

#[tokio::test]
async fn oversized_error_cannot_erase_uncertain_cleanup_and_reopen_admission() {
    let (agent, backend, storage) = probe(false).await;
    backend.fail_close.store(true, Ordering::SeqCst);
    backend.suppress_terminal.store(true, Ordering::SeqCst);
    *backend.execution_attachment.lock().unwrap() = ProviderSessionState::CleanupRequired;
    *backend.execution_error.lock().unwrap() = Some(AgentError::OperationAndCleanupFailure {
        operation_error: Box::new(AgentError::Protocol("x".repeat(1024 * 1024))),
        cleanup_error: Box::new(AgentError::AuditAndCleanupFailure),
    });
    let result = agent.invoke(input("uncertain"), close_action()).await;
    assert_eq!(
        result,
        Err(AgentError::ExecutionObservation {
            error: Box::new(AgentError::CleanupUncertain),
            execution_result: Some(Box::new(Err(AgentError::DiagnosticLimit))),
        })
    );
    assert_eq!(storage.snapshot().invocations[0].result, Some(result));
    assert_eq!(
        agent.invoke(input("blocked"), close_action()).await,
        Err(AgentError::Closed)
    );
    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
    backend.fail_close.store(false, Ordering::SeqCst);
    agent.close(close_action()).await.unwrap();
}
