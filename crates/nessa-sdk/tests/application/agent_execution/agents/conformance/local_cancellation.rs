//! A backend's local settlement needs the same admitted work's stop evidence.
use super::*;
use nessa_sdk::application::agent_execution::providers::ResourceCleanup;

#[tokio::test]
async fn local_cancellation_requires_owned_stop_and_preserves_explicit_closer() {
    for mode in Mode::ALL {
        for stopped in [false, true] {
            let (agent, backend, storage) = workflow().await;
            *backend.execution_report.lock().unwrap() = Some(ExecutionReport::cancelled_locally(
                CleanupReport::confirmed(CloseOutcome { forced: false }),
            ));
            let (release, wait) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(wait);
            let running = mode.start(agent.clone(), request("local-cancel"));
            bounded(backend.dispatched.notified()).await;
            if stopped {
                bounded(agent.close(actor())).await.unwrap();
            } else {
                release.send(()).unwrap();
            }
            let result = bounded(running).await.unwrap();
            let snapshot = storage.snapshot();
            let record = &snapshot.invocations[0];
            if stopped {
                assert_eq!(result, Ok(ExecutionOutcome::Cancelled));
                let stop = record.local_cancellation.as_ref().unwrap();
                assert_eq!(stop.cause, SchedulingCause::SessionClosed);
                assert_eq!(stop.actor, Some(actor()));
                assert!(record.provider_report.is_some());
            } else {
                assert!(
                    matches!(result, Err(AgentError::ExecutionObservation { .. })),
                    "{result:?}"
                );
                assert!(record.provider_report.is_none());
                assert!(record.local_cancellation.is_none());
                assert_eq!(record.result, Some(result));
            }
            let expected_stop = record.local_cancellation.clone();
            let restored_storage = MemoryStorage::default();
            restored_storage.0.lock().unwrap().snapshot = Some(snapshot);
            let (restored, _, restored_storage) = workflow_from_storage(restored_storage).await;
            assert_eq!(
                restored_storage.snapshot().invocations[0].local_cancellation,
                expected_stop
            );
            bounded(restored.close(actor())).await.unwrap();
            bounded(agent.close(actor())).await.unwrap();
        }
    }
}

#[tokio::test]
async fn unowned_local_cancellation_never_hides_cleanup_or_audit_failure() {
    for mode in Mode::ALL {
        for confirmed in [false, true] {
            for audit_failed in [false, true] {
                let (agent, backend, storage) = workflow().await;
                let cleanup = CleanupReport::new(
                    if confirmed {
                        ResourceCleanup::Confirmed(CloseOutcome { forced: false })
                    } else {
                        ResourceCleanup::Unconfirmed(AgentError::CleanupUncertain)
                    },
                    if audit_failed {
                        Err(AgentError::AuditFailure)
                    } else {
                        Ok(())
                    },
                );
                *backend.execution_report.lock().unwrap() =
                    Some(ExecutionReport::cancelled_locally(cleanup.clone()));
                let cleanup_error = cleanup.clone().into_result().err();
                *backend.cleanup.lock().unwrap() = cleanup;
                let result = bounded(mode.start(agent.clone(), request("forged-cancel")))
                    .await
                    .unwrap();
                let error = result.as_ref().unwrap_err();
                let AgentError::ExecutionObservation {
                    error: observed,
                    execution_result: Some(projected),
                } = error
                else {
                    panic!("{error:?}")
                };
                assert!(matches!(projected.as_ref(), Err(AgentError::Protocol(_))));
                let protocol = projected.as_ref().as_ref().unwrap_err().clone();
                assert_eq!(
                    observed.as_ref(),
                    &match cleanup_error {
                        Some(cleanup_error) => AgentError::OperationAndCleanupFailure {
                            operation_error: Box::new(protocol),
                            cleanup_error: Box::new(cleanup_error)
                        },
                        None => protocol,
                    }
                );
                if !confirmed {
                    assert_eq!(
                        bounded(agent.invoke(request("blocked"), actor())).await,
                        Err(AgentError::Closed)
                    );
                    assert_eq!(backend.executions.lock().unwrap().len(), 1);
                }
                let snapshot = storage.snapshot();
                assert_eq!(snapshot.invocations[0].result, Some(result));
                assert!(snapshot.invocations[0].provider_report.is_none());
                assert!(snapshot.invocations[0].local_outcome.is_none());
                assert!(snapshot.invocations[0].local_cancellation.is_none());
                *backend.cleanup.lock().unwrap() =
                    CleanupReport::confirmed(CloseOutcome { forced: false });
                let _ = bounded(agent.close(actor())).await;
            }
        }
    }
}

#[tokio::test]
async fn automatic_stop_authorizes_local_settlement_without_erasing_observation_failure() {
    for mode in Mode::ALL {
        let (agent, backend, storage) = workflow().await;
        *backend.execution_report.lock().unwrap() = Some(ExecutionReport::cancelled_locally(
            CleanupReport::confirmed(CloseOutcome { forced: false }),
        ));
        let (_release, wait) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(wait);
        let running = mode.start(agent.clone(), request("automatic-stop"));
        bounded(backend.dispatched.notified()).await;
        backend
            .output
            .lock()
            .unwrap()
            .send(Some(ExecutionEvent::new(
                ExecutionId::new("foreign-execution").unwrap(),
                ExecutionUpdate::Message(MessageChunk::text("invalid target")),
            )))
            .unwrap();
        let result = bounded(running).await.unwrap();
        assert!(result.is_err());
        let saved = storage.snapshot();
        let stop = saved.invocations[0].local_cancellation.as_ref().unwrap();
        assert_eq!(stop.cause, SchedulingCause::RunnerStopped);
        assert_eq!(stop.actor, None);
        assert!(saved.invocations[0].provider_report.is_some());
        assert_eq!(saved.invocations[0].result, Some(result));
        bounded(agent.close(actor())).await.unwrap();
    }
}
