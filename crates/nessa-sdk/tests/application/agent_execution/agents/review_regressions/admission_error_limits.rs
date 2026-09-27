//! Diagnostic bounds preserve rejection of work that emitted no terminal event.
use super::*;
use std::mem::discriminant;

#[tokio::test]
async fn oversized_admission_errors_settle_without_terminal_for_direct_and_queued_work() {
    for queued in [false, true] {
        for error in [
            AgentError::InvalidInput("é".repeat(1024 * 1024)),
            AgentError::Unsupported("é".repeat(1024 * 1024)),
            AgentError::Configuration("é".repeat(1024 * 1024)),
            AgentError::Protocol("é".repeat(1024 * 1024)),
            AgentError::Transport("é".repeat(1024 * 1024)),
        ] {
            let (agent, backend, storage) = probe(false).await;
            backend.suppress_terminal.store(true, Ordering::SeqCst);
            backend.execution_rejected.store(true, Ordering::SeqCst);
            let expected_variant = discriminant(&error);
            *backend.execution_error.lock().unwrap() = Some(error);
            let result = timeout(Duration::from_secs(3), async {
                if queued {
                    agent
                        .enqueue(input("rejected"), actor())
                        .await
                        .unwrap()
                        .wait()
                        .await
                } else {
                    agent.invoke(input("rejected"), actor()).await
                }
            })
            .await
            .expect("an admission rejection has no terminal event to await");
            let error = result.as_ref().unwrap_err();
            assert_eq!(discriminant(error), expected_variant);
            let text = match error {
                AgentError::InvalidInput(text)
                | AgentError::Unsupported(text)
                | AgentError::Configuration(text)
                | AgentError::Protocol(text)
                | AgentError::Transport(text) => text,
                _ => unreachable!(),
            };
            assert!(text.ends_with(" [diagnostic truncated]"));
            assert!(text.capacity() <= 4096);
            let saved = storage.snapshot();
            assert_eq!(saved.invocations[0].result, Some(result));
            assert!(saved.invocations[0].events.is_empty());
            assert!(saved.invocations[0].provider_report.is_none());
            assert_eq!(backend.closes.load(Ordering::SeqCst), 0);
            // Rejection leaves the same attachment available for subsequent work.
            *backend.execution_error.lock().unwrap() = None;
            backend.execution_rejected.store(false, Ordering::SeqCst);
            backend.suppress_terminal.store(false, Ordering::SeqCst);
            assert_eq!(
                timeout(Duration::from_secs(3), async {
                    agent
                        .enqueue(input("following"), actor())
                        .await
                        .unwrap()
                        .wait()
                        .await
                })
                .await
                .unwrap(),
                Ok(ExecutionOutcome::Completed)
            );
            assert_eq!(storage.snapshot().invocations.len(), 2);
            agent.close(actor()).await.unwrap();
        }
    }
}
