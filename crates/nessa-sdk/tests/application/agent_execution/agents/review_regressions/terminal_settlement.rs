//! A terminal observation and a later failed settlement remain separate evidence.
use super::*;

#[tokio::test]
async fn terminal_observation_with_failed_settlement_cleans_up_and_preserves_both_facts() {
    for reported in [
        ExecutionOutcome::Completed,
        ExecutionOutcome::Cancelled,
        ExecutionOutcome::Refused,
        ExecutionOutcome::OutputLimit,
        ExecutionOutcome::RequestLimit,
    ] {
        for failure in [
            AgentError::Transport("late failure".into()),
            AgentError::AuditFailure,
        ] {
            for uncertain in [false, true] {
                let (agent, backend, storage) = probe(false).await;
                backend.suppress_terminal.store(true, Ordering::SeqCst);
                backend.fail_close.store(uncertain, Ordering::SeqCst);
                *backend.execution_error.lock().unwrap() = Some(failure.clone());
                let (release, wait) = oneshot::channel();
                *backend.execution_gate.lock().unwrap() = Some(wait);
                let mut updates = agent.subscribe();
                let receipt = agent
                    .enqueue(input("terminal-error"), actor())
                    .await
                    .unwrap();
                backend.executing.notified().await;
                backend
                    .sender
                    .lock()
                    .unwrap()
                    .send(ExecutionEvent::new(
                        ExecutionId::new("terminal-error").unwrap(),
                        ExecutionUpdate::Finished(reported),
                    ))
                    .unwrap();
                assert_eq!(
                    updates.next().await.unwrap().unwrap().update(),
                    &ExecutionUpdate::Finished(reported)
                );
                release.send(()).unwrap();
                let result = timeout(Duration::from_secs(3), receipt.wait())
                    .await
                    .unwrap();
                let protocol = AgentError::Protocol(
                    "terminal observation contradicts execution settlement".into(),
                );
                let expected = Err(AgentError::ExecutionObservation {
                    error: Box::new(if uncertain {
                        AgentError::OperationAndCleanupFailure {
                            operation_error: Box::new(protocol),
                            cleanup_error: Box::new(AgentError::CleanupUncertain),
                        }
                    } else {
                        protocol
                    }),
                    execution_result: Some(Box::new(Err(failure.clone()))),
                });
                assert_eq!(result, expected);
                let snapshot = storage.snapshot();
                assert_eq!(snapshot.invocations[0].result, Some(result));
                assert_eq!(
                    snapshot.invocations[0].events[0].update(),
                    &ExecutionUpdate::Finished(reported)
                );
                assert_eq!(
                    *backend.close_requests.lock().unwrap(),
                    vec![SessionCloseRequest::ExecutionFailed]
                );
                assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
                if uncertain {
                    assert!(matches!(
                        agent.enqueue(input("blocked"), actor()).await,
                        Err(AgentError::Closed)
                    ));
                    backend.fail_close.store(false, Ordering::SeqCst);
                    agent.close(actor()).await.unwrap();
                    reattach_after_explicit_close(&agent).await;
                } else {
                    recover_after_automatic_stop(&agent).await;
                }
                *backend.execution_error.lock().unwrap() = None;
                backend.suppress_terminal.store(false, Ordering::SeqCst);
                assert_eq!(
                    agent
                        .enqueue(input("next"), actor())
                        .await
                        .unwrap()
                        .wait()
                        .await,
                    Ok(ExecutionOutcome::Completed)
                );
                agent.close(actor()).await.unwrap();
            }
        }
    }
}
