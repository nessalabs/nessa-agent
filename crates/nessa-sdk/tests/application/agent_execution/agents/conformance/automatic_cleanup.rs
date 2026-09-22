//! Automatic cleanup belongs in the caller's saved result while provider facts remain exact.
use super::*;
use nessa_sdk::application::agent_execution::providers::ResourceCleanup;

#[tokio::test]
async fn automatic_cleanup_failures_survive_every_invocation_result_and_restoration() {
    let outcomes = [
        Some(Ok(ExecutionOutcome::Completed)),
        Some(Ok(ExecutionOutcome::OutputLimit)),
        Some(Ok(ExecutionOutcome::RequestLimit)),
        Some(Ok(ExecutionOutcome::Refused)),
        Some(Ok(ExecutionOutcome::Cancelled)),
        Some(Err(AgentError::Provider {
            code: -32077,
            diagnostic: None,
        })),
        None,
    ];
    for mode in Mode::ALL {
        for outcome in &outcomes {
            for independent_failure in [
                None,
                Some(AgentError::Protocol("settlement audit failed".into())),
            ] {
                for confirmed in [false, true] {
                    for audit_failed in [false, true] {
                        let (agent, backend, storage) = workflow().await;
                        let report = ExecutionReport::new(
                            outcome.clone(),
                            independent_failure.clone(),
                            ProviderSessionState::CleanupRequired,
                        );
                        let cleanup = CleanupReport::new(
                            if confirmed {
                                ResourceCleanup::Confirmed(CloseOutcome { forced: false })
                            } else {
                                ResourceCleanup::Unconfirmed(AgentError::CleanupUncertain)
                            },
                            if audit_failed {
                                Err(AgentError::Protocol("cleanup audit failed".into()))
                            } else {
                                Ok(())
                            },
                        );
                        *backend.execution_report.lock().unwrap() = Some(report.clone());
                        *backend.cleanup.lock().unwrap() = cleanup.clone();
                        let original = report.clone().into_result();
                        let expected = match cleanup.into_result() {
                            Ok(_) => original,
                            Err(error) => Err(AgentError::ExecutionObservation {
                                error: Box::new(error),
                                execution_result: Some(Box::new(original)),
                            }),
                        };
                        let result = bounded(mode.start(agent.clone(), request("cleanup")))
                            .await
                            .unwrap();
                        assert_eq!(result, expected, "{mode:?} {outcome:?} confirmed={confirmed} audit_failed={audit_failed}");
                        let saved = storage.snapshot();
                        assert_eq!(saved.invocations[0].result, Some(expected.clone()));
                        assert_eq!(saved.invocations[0].provider_report, Some(report.clone()));
                        assert_eq!(
                            backend.shutdowns.lock().unwrap().as_slice(),
                            &[SessionCloseRequest::ExecutionFailed]
                        );
                        // Load an independent copy through the same public initialization boundary.
                        let restored_storage = MemoryStorage::default();
                        restored_storage.0.lock().unwrap().snapshot = Some(saved);
                        let (restored, _, _) =
                            workflow_from_storage(restored_storage.clone()).await;
                        let restored_snapshot = restored_storage.snapshot();
                        assert_eq!(restored_snapshot.invocations[0].result, Some(expected));
                        assert_eq!(
                            restored_snapshot.invocations[0].provider_report,
                            Some(report)
                        );
                        bounded(restored.close(actor())).await.unwrap();
                        // Finish fixture resource ownership even in the deliberately failed case.
                        *backend.cleanup.lock().unwrap() =
                            CleanupReport::confirmed(CloseOutcome { forced: false });
                        let _ = bounded(agent.close(actor())).await;
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn cleanup_required_recovers_only_after_confirmed_resources_and_audit() {
    for mode in Mode::ALL {
        for confirmed in [false, true] {
            for audit_failed in [false, true] {
                let (agent, backend, storage) = workflow().await;
                *backend.execution_report.lock().unwrap() = Some(ExecutionReport::new(
                    Some(Ok(ExecutionOutcome::Completed)),
                    None,
                    ProviderSessionState::CleanupRequired,
                ));
                *backend.cleanup.lock().unwrap() = CleanupReport::new(
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
                let first = bounded(mode.start(agent.clone(), request("first")))
                    .await
                    .unwrap();
                assert_eq!(first.is_ok(), confirmed && !audit_failed);
                assert!(storage.snapshot().invocations[0].provider_report.is_some());
                *backend.execution_report.lock().unwrap() = None;
                let next = bounded(mode.start(agent.clone(), request("next")))
                    .await
                    .unwrap();
                if confirmed && !audit_failed {
                    assert_eq!(next, Ok(ExecutionOutcome::Completed), "{mode:?}");
                    assert_eq!(backend.executions.lock().unwrap().len(), 2);
                } else {
                    assert_eq!(
                        next,
                        Err(if confirmed && audit_failed {
                            AgentError::AuditFailure
                        } else {
                            AgentError::Closed
                        }),
                        "{mode:?}"
                    );
                    assert_eq!(backend.executions.lock().unwrap().len(), 1);
                }
                *backend.cleanup.lock().unwrap() =
                    CleanupReport::confirmed(CloseOutcome { forced: false });
                let _ = bounded(agent.close(actor())).await;
            }
        }
    }
}

#[tokio::test]
async fn automatic_stop_keeps_native_steering_cause_across_retry_and_restoration() {
    for audit_failed in [false, true] {
        let (agent, backend, storage) = workflow().await;
        let (release_execution, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        *backend.execution_report.lock().unwrap() = Some(ExecutionReport::new(
            Some(Err(AgentError::Provider {
                code: -32077,
                diagnostic: None,
            })),
            None,
            ProviderSessionState::CleanupRequired,
        ));
        if audit_failed {
            *backend.cleanup.lock().unwrap() = CleanupReport::new(
                ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                Err(AgentError::AuditFailure),
            );
        }
        let running = Mode::Direct.start(agent.clone(), request("target"));
        bounded(backend.dispatched.notified()).await;
        let (_release_steering, gate) = oneshot::channel();
        *backend.steering_gate.lock().unwrap() = Some(gate);
        let steering = tokio::spawn({
            let agent = agent.clone();
            async move { agent.steer(request("correction"), actor()).await }
        });
        bounded(backend.steering_admitted.notified()).await;
        // Native delivery is pending before the independent execution reports
        // that this provider generation must stop. No explicit closer exists.
        release_execution.send(()).unwrap();
        let error = bounded(steering).await.unwrap().err().unwrap();
        assert!(bounded(running).await.unwrap().is_err());
        assert_eq!(
            bounded(agent.steer(request("correction"), actor()))
                .await
                .err(),
            Some(error.clone())
        );
        let saved = storage.snapshot();
        let record = &saved.invocations[1];
        let stop = record.scheduling.last().unwrap();
        assert_eq!(stop.before, Some(InvocationStage::Queued));
        assert_eq!(stop.stage, InvocationStage::Cancelled);
        assert_eq!(stop.cause, SchedulingCause::RunnerStopped);
        assert_eq!(stop.actor, None);
        assert_eq!(stop.target, Some(ExecutionId::new("target").unwrap()));
        assert_eq!(record.result, Some(Err(error.clone())));
        assert!(record.provider_report.is_none());
        assert!(record.events.is_empty());
        assert_eq!(backend.steering.lock().unwrap().len(), 1);
        assert_eq!(backend.executions.lock().unwrap().len(), 1);
        let expected = record.scheduling.clone();
        let restored_storage = MemoryStorage::default();
        restored_storage.0.lock().unwrap().snapshot = Some(saved);
        let (restored, restored_backend, restored_storage) =
            workflow_from_storage(restored_storage).await;
        assert_eq!(
            bounded(restored.steer(request("correction"), actor()))
                .await
                .err(),
            Some(error)
        );
        assert_eq!(
            restored_storage.snapshot().invocations[1].scheduling,
            expected
        );
        assert!(restored_backend.steering.lock().unwrap().is_empty());
        assert!(restored_backend.executions.lock().unwrap().is_empty());
        bounded(restored.close(actor())).await.unwrap();
        *backend.cleanup.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
        let _ = bounded(agent.close(actor())).await;
    }
}
