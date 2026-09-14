//! Queue progress follows provider health, independently of an operation's diagnostic.
use super::*;
use nessa_sdk::application::agent_execution::providers::ResourceCleanup;

#[tokio::test]
async fn native_error_queue_policy_uses_provider_state_and_preserves_saved_receipts() {
    for diagnostic in [
        AgentError::Unsupported("steering rejected".into()),
        AgentError::InvalidInput("steering rejected".into()),
        AgentError::Busy,
        AgentError::Provider { code: -32077 },
        AgentError::Protocol("steering rejected".into()),
    ] {
        for provider_state in [
            ProviderSessionState::Usable,
            ProviderSessionState::CleanupRequired,
            ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                forced: false,
            })),
            ProviderSessionState::CleanupReported(CleanupReport::new(
                ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                Err(AgentError::AuditFailure),
            )),
        ] {
            let cleanup_required = matches!(provider_state, ProviderSessionState::CleanupRequired);
            let reported_audit_failure = matches!(&provider_state, ProviderSessionState::CleanupReported(report) if report.audit().is_err());
            let stops_waiting = cleanup_required || reported_audit_failure;
            for (audit_failed, cleanup_unconfirmed) in
                [(false, false), (true, false), (false, true)]
            {
                if (audit_failed || cleanup_unconfirmed) && !cleanup_required {
                    continue;
                }
                let (agent, backend, storage) = probe(false).await;
                let (release_execution, gate) = oneshot::channel();
                *backend.execution_gate.lock().unwrap() = Some(gate);
                let running = tokio::spawn({
                    let agent = agent.clone();
                    async move { agent.invoke(input("target"), actor()).await }
                });
                timeout(Duration::from_secs(2), backend.executing.notified())
                    .await
                    .unwrap();
                let queued = agent.enqueue(input("waiting"), actor()).await.unwrap();
                *backend.control_error.lock().unwrap() = Some(diagnostic.clone());
                *backend.control_attachment.lock().unwrap() = provider_state.clone();
                if cleanup_unconfirmed {
                    *backend.cleanup_report.lock().unwrap() =
                        Some(CleanupReport::unconfirmed(AgentError::CleanupUncertain));
                }
                if audit_failed {
                    *backend.cleanup_report.lock().unwrap() = Some(CleanupReport::new(
                        ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                        Err(AgentError::AuditFailure),
                    ));
                }
                let steering_error = timeout(
                    Duration::from_secs(2),
                    agent.steer(input("correction"), actor()),
                )
                .await
                .unwrap()
                .err()
                .unwrap();
                if audit_failed || cleanup_unconfirmed || reported_audit_failure {
                    assert_eq!(
                        steering_error,
                        AgentError::OperationAndCleanupFailure {
                            operation_error: Box::new(diagnostic.clone()),
                            cleanup_error: Box::new(if audit_failed || reported_audit_failure {
                                AgentError::AuditFailure
                            } else {
                                AgentError::CleanupUncertain
                            }),
                        }
                    );
                }
                if stops_waiting {
                    assert_eq!(
                        timeout(Duration::from_secs(2), queued.wait())
                            .await
                            .unwrap(),
                        Err(AgentError::Closed),
                        "{diagnostic:?}, audit_failed={audit_failed}"
                    );
                    // This fixture's target emitted its terminal event already;
                    // release the separately held provider result after checking
                    // the waiting receipt settles without that release.
                    let _ = release_execution.send(());
                    let _ = timeout(Duration::from_secs(2), running)
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(backend.closes.load(Ordering::SeqCst) > 0, cleanup_required);
                    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
                } else {
                    assert_eq!(steering_error, diagnostic);
                    release_execution.send(()).unwrap();
                    assert_eq!(running.await.unwrap(), Ok(ExecutionOutcome::Completed));
                    assert_eq!(
                        timeout(Duration::from_secs(2), queued.wait())
                            .await
                            .unwrap(),
                        Ok(ExecutionOutcome::Completed),
                        "{diagnostic:?}"
                    );
                    assert_eq!(backend.closes.load(Ordering::SeqCst), 0);
                    assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
                }
                assert_eq!(
                    agent.steer(input("correction"), actor()).await.err(),
                    Some(steering_error.clone())
                );
                let saved = storage.snapshot();
                let waiting = &saved.invocations[1];
                let stop = waiting.scheduling.last().unwrap();
                assert_eq!(
                    stop.stage,
                    if stops_waiting {
                        InvocationStage::Cancelled
                    } else {
                        InvocationStage::Settled
                    }
                );
                assert_eq!(
                    stop.cause,
                    if stops_waiting {
                        SchedulingCause::RunnerStopped
                    } else {
                        SchedulingCause::ExecutionSettled
                    }
                );
                assert_eq!(stop.actor, None);
                assert_eq!(
                    saved.invocations[2].result,
                    Some(Err(steering_error.clone()))
                );
                let correction = &saved.invocations[2];
                let correction_stop = correction.scheduling.last().unwrap();
                let usable = matches!(provider_state, ProviderSessionState::Usable);
                assert_eq!(
                    correction_stop.stage,
                    if usable {
                        InvocationStage::Settled
                    } else {
                        InvocationStage::Cancelled
                    }
                );
                assert_eq!(
                    correction_stop.cause,
                    if usable {
                        SchedulingCause::DispatchFailed
                    } else {
                        SchedulingCause::RunnerStopped
                    }
                );
                assert_eq!(
                    correction_stop.target,
                    Some(ExecutionId::new("target").unwrap())
                );
                assert_eq!(correction_stop.actor, None);
                let saved_queue = waiting.scheduling.clone();
                let saved_steering = correction.scheduling.clone();
                let restored_storage = MemoryStorage::default();
                restored_storage.0.lock().unwrap().snapshot = Some(saved);
                let (restored, restored_backend) =
                    probe_with_manager(false, restored_storage.manager().await).await;
                assert_eq!(
                    restored.steer(input("correction"), actor()).await.err(),
                    Some(steering_error)
                );
                let restored_queue = restored.enqueue(input("waiting"), actor()).await.unwrap();
                assert_eq!(
                    restored_queue.wait().await,
                    if stops_waiting {
                        Err(AgentError::Closed)
                    } else {
                        Ok(ExecutionOutcome::Completed)
                    }
                );
                assert_eq!(
                    restored_storage.snapshot().invocations[1].scheduling,
                    saved_queue
                );
                assert_eq!(
                    restored_storage.snapshot().invocations[2].scheduling,
                    saved_steering
                );
                assert_eq!(restored_backend.executions.load(Ordering::SeqCst), 0);
                assert_eq!(restored_backend.steers.load(Ordering::SeqCst), 0);
                restored.close(actor()).await.unwrap();
                *backend.cleanup_report.lock().unwrap() = None;
                let _ = agent.close(actor()).await;
            }
        }
    }
}

#[tokio::test]
async fn failed_cleanup_audit_settles_waiting_receipts_from_every_provider_operation() {
    // None represents the execution's own terminal provider report.
    for source in [
        None,
        Some(ProviderControl::Answer),
        Some(ProviderControl::CancelPermission),
    ] {
        for audit_failed in [false, true] {
            let (agent, backend, storage) = probe(false).await;
            let (release_execution, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            let running = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("active"), actor()).await }
            });
            timeout(Duration::from_secs(2), backend.executing.notified())
                .await
                .unwrap();
            let queued = agent.enqueue(input("waiting"), actor()).await.unwrap();
            let state = ProviderSessionState::CleanupReported(CleanupReport::new(
                ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                if audit_failed {
                    Err(AgentError::AuditFailure)
                } else {
                    Ok(())
                },
            ));
            match source {
                Some(operation) => {
                    *backend.control_attachment.lock().unwrap() = state;
                    *backend.control_error.lock().unwrap() = Some(AgentError::StalePermission);
                    assert_eq!(
                        invoke_control(&agent, operation).await,
                        Err(AgentError::StalePermission)
                    );
                }
                None => *backend.execution_attachment.lock().unwrap() = state,
            }
            release_execution.send(()).unwrap();
            let _ = timeout(Duration::from_secs(2), running)
                .await
                .unwrap()
                .unwrap();
            let expected = if audit_failed {
                Err(AgentError::Closed)
            } else {
                Ok(ExecutionOutcome::Completed)
            };
            assert_eq!(
                timeout(Duration::from_secs(2), queued.wait())
                    .await
                    .unwrap(),
                expected,
                "{source:?}, audit_failed={audit_failed}"
            );
            assert_eq!(
                backend.executions.load(Ordering::SeqCst),
                if audit_failed { 1 } else { 2 }
            );
            // This must settle without an explicit close or another submission
            // repairing the stalled queue as a side effect.
            let saved = storage.snapshot();
            let record = &saved.invocations[1];
            let transition = record.scheduling.last().unwrap();
            assert_eq!(
                transition.before,
                Some(if audit_failed {
                    InvocationStage::Queued
                } else {
                    InvocationStage::Running
                })
            );
            assert_eq!(
                transition.stage,
                if audit_failed {
                    InvocationStage::Cancelled
                } else {
                    InvocationStage::Settled
                }
            );
            assert_eq!(
                transition.cause,
                if audit_failed {
                    SchedulingCause::RunnerStopped
                } else {
                    SchedulingCause::ExecutionSettled
                }
            );
            assert_eq!(transition.actor, None);
            assert_eq!(transition.target, None);
            let expected_history = record.scheduling.clone();
            assert_eq!(
                agent
                    .enqueue(input("waiting"), actor())
                    .await
                    .unwrap()
                    .wait()
                    .await,
                expected
            );
            let restored_storage = MemoryStorage::default();
            restored_storage.0.lock().unwrap().snapshot = Some(saved);
            let (restored, restored_backend) =
                probe_with_manager(false, restored_storage.manager().await).await;
            assert_eq!(
                restored
                    .enqueue(input("waiting"), actor())
                    .await
                    .unwrap()
                    .wait()
                    .await,
                expected
            );
            assert_eq!(
                restored_storage.snapshot().invocations[1].scheduling,
                expected_history
            );
            assert_eq!(restored_backend.executions.load(Ordering::SeqCst), 0);
            restored.close(actor()).await.unwrap();
            assert_eq!(
                agent.close(actor()).await,
                if audit_failed {
                    Err(AgentError::AuditFailure)
                } else {
                    Ok(CloseOutcome { forced: false })
                }
            );
        }
    }
}
