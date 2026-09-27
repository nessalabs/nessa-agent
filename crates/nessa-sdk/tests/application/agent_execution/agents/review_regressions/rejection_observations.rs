//! Provider observations contradict admission rejection regardless of arrival order.
mod control_fence;
mod ready_stream;
use super::*;
use nessa_sdk::infrastructure::session_storage::LocalFileStorage;

fn observations() -> [ExecutionUpdate; 3] {
    [
        ExecutionUpdate::Message(MessageChunk::text("observed before rejection")),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("observed-tool").unwrap(),
            None,
            None,
            None,
            None,
            None,
        )),
        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
    ]
}

#[tokio::test]
async fn observations_before_rejection_preserve_evidence_and_require_protocol_cleanup() {
    for update in observations() {
        for cleanup_failure in [
            None,
            Some(AgentError::AuditFailure),
            Some(AgentError::CleanupUncertain),
        ] {
            let (agent, backend, storage) = probe(false).await;
            backend.execution_rejected.store(true, Ordering::SeqCst);
            backend.suppress_terminal.store(true, Ordering::SeqCst);
            let rejection = AgentError::InvalidInput("not admitted".into());
            *backend.execution_error.lock().unwrap() = Some(rejection.clone());
            *backend.late_output.lock().unwrap() = Some(update.clone());
            *backend.cleanup_report.lock().unwrap() = cleanup_failure.as_ref().map(|error| {
                if error == &AgentError::AuditFailure {
                    CleanupReport::new(
                        ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                        Err(error.clone()),
                    )
                } else {
                    CleanupReport::unconfirmed(error.clone())
                }
            });
            let (release, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            let (cleanup_release, cleanup_gate) = oneshot::channel();
            *backend.close_gate.lock().unwrap() = Some(cleanup_gate);
            let mut updates = agent.subscribe();
            let invocation = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("rejected"), actor()).await }
            });
            let observed = timeout(Duration::from_secs(2), updates.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(observed.execution_id().as_str(), "rejected");
            assert_eq!(observed.update(), &update);
            release.send(()).unwrap();
            timeout(Duration::from_secs(2), backend.closing.notified())
                .await
                .unwrap();
            assert!(
                !invocation.is_finished(),
                "rejection cannot bypass pending cleanup"
            );
            cleanup_release.send(()).unwrap();
            let result = timeout(Duration::from_secs(2), invocation)
                .await
                .unwrap()
                .unwrap();
            let Err(AgentError::ExecutionObservation {
                error,
                execution_result,
            }) = &result
            else {
                panic!("missing protocol failure: {result:?}")
            };
            assert_eq!(execution_result.as_deref(), Some(&Err(rejection)));
            match cleanup_failure.as_ref() {
                None => assert!(matches!(error.as_ref(), AgentError::Protocol(_))),
                Some(expected) => assert!(
                    matches!(error.as_ref(), AgentError::OperationAndCleanupFailure {operation_error, cleanup_error} if matches!(operation_error.as_ref(), AgentError::Protocol(_)) && cleanup_error.as_ref() == expected)
                ),
            }
            assert_eq!(
                *backend.close_requests.lock().unwrap(),
                vec![SessionCloseRequest::ExecutionFailed]
            );
            let saved = storage.snapshot();
            let record = &saved.invocations[0];
            assert_eq!(record.events, vec![observed]);
            assert_eq!(record.result, Some(result));
            assert!(
                record.provider_report.is_none(),
                "rejection must not fabricate a provider outcome"
            );
            assert!(record.local_outcome.is_none());
            // Both checked file storage and an unchecked custom adapter preserve
            // the observations together with their local protocol failure.
            let directory = tempfile::tempdir().unwrap();
            let file = LocalFileStorage::new(directory.path().join("private")).unwrap();
            let lease = file.open(saved.id.clone()).await.unwrap();
            lease.save(saved.clone()).await.unwrap();
            let loaded = lease.load().await.unwrap().unwrap();
            assert_eq!(loaded.invocations[0].events, record.events);
            assert_eq!(loaded.invocations[0].result, record.result);
            assert!(loaded.invocations[0].provider_report.is_none());
            let imported = MemoryStorage::default();
            imported.0.lock().unwrap().snapshot = Some(loaded);
            let (restored, _) = probe_with_manager(false, imported.manager().await).await;
            restored.close(actor()).await.unwrap();
            if cleanup_failure.is_some() {
                assert!(agent.invoke(input("blocked"), actor()).await.is_err());
                assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
            }
            *backend.cleanup_report.lock().unwrap() = None;
            *backend.execution_error.lock().unwrap() = None;
            *backend.late_output.lock().unwrap() = None;
            backend.execution_rejected.store(false, Ordering::SeqCst);
            backend.suppress_terminal.store(false, Ordering::SeqCst);
            if cleanup_failure == Some(AgentError::AuditFailure) {
                assert_eq!(agent.close(actor()).await, Err(AgentError::AuditFailure));
                continue;
            }
            if cleanup_failure.is_some() {
                agent.close(actor()).await.unwrap();
                reattach_after_explicit_close(&agent).await;
            } else {
                recover_after_automatic_stop(&agent).await;
            }
            assert_eq!(
                agent.invoke(input("restored"), actor()).await,
                Ok(ExecutionOutcome::Completed)
            );
            agent.close(actor()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn observations_after_rejection_never_reenter_the_rejected_history() {
    for update in observations() {
        let (agent, backend, storage) = probe(false).await;
        backend.execution_rejected.store(true, Ordering::SeqCst);
        backend.suppress_terminal.store(true, Ordering::SeqCst);
        let rejection = AgentError::InvalidInput("not admitted".into());
        *backend.execution_error.lock().unwrap() = Some(rejection.clone());
        assert_eq!(
            agent.invoke(input("rejected"), actor()).await,
            Err(rejection.clone())
        );
        assert_eq!(backend.closes.load(Ordering::SeqCst), 0);
        backend
            .sender
            .lock()
            .unwrap()
            .send(ExecutionEvent::new(
                ExecutionId::new("rejected").unwrap(),
                update,
            ))
            .unwrap();
        backend.execution_rejected.store(false, Ordering::SeqCst);
        backend.suppress_terminal.store(false, Ordering::SeqCst);
        *backend.execution_error.lock().unwrap() = None;
        let result = agent.invoke(input("later"), actor()).await;
        assert!(
            matches!(&result, Err(AgentError::Protocol(_))),
            "{result:?}"
        );
        assert_eq!(
            *backend.close_requests.lock().unwrap(),
            vec![SessionCloseRequest::ExecutionFailed]
        );
        assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
        let saved = storage.snapshot();
        assert!(saved.invocations[0].events.is_empty());
        assert!(saved.invocations[0].provider_report.is_none());
        assert_eq!(saved.invocations[0].result, Some(Err(rejection)));
        agent.close(actor()).await.unwrap();
    }
}

#[tokio::test]
async fn same_poll_observation_and_rejection_are_drained_before_next_dispatch() {
    for mode in [
        SubmissionMode::Immediate,
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
    ] {
        for update in observations() {
            let (agent, backend, storage) = probe(false).await;
            backend.execution_rejected.store(true, Ordering::SeqCst);
            backend.suppress_terminal.store(true, Ordering::SeqCst);
            *backend.late_output.lock().unwrap() = Some(update.clone());
            let rejection = AgentError::InvalidInput("same-poll rejection".into());
            *backend.execution_error.lock().unwrap() = Some(rejection.clone());
            let (cleanup_release, cleanup_gate) = oneshot::channel();
            *backend.close_gate.lock().unwrap() = Some(cleanup_gate);
            let mut updates = agent.subscribe();
            // Probe queues this event and returns Rejected in one execute poll.
            // The biased select sees the ready reply before polling the event stream.
            let invocation = tokio::spawn({
                let agent = agent.clone();
                async move {
                    match mode {
                        SubmissionMode::Immediate => {
                            agent.invoke(input("same-poll"), actor()).await
                        }
                        SubmissionMode::Queued => {
                            agent
                                .enqueue(input("same-poll"), actor())
                                .await?
                                .wait()
                                .await
                        }
                        SubmissionMode::BoundarySteering => {
                            agent
                                .enqueue_steering(input("same-poll"), actor())
                                .await?
                                .wait()
                                .await
                        }
                        SubmissionMode::Steering => {
                            unreachable!("native injection has a separate active history")
                        }
                    }
                }
            });
            timeout(Duration::from_secs(2), backend.closing.notified())
                .await
                .unwrap();
            assert!(!invocation.is_finished());
            assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
            // A competing invocation cannot reach the provider during cleanup.
            assert!(agent.invoke(input("too-early"), actor()).await.is_err());
            assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
            let observed = timeout(Duration::from_secs(2), updates.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(observed.execution_id().as_str(), "same-poll");
            assert_eq!(observed.update(), &update);
            cleanup_release.send(()).unwrap();
            let result = timeout(Duration::from_secs(2), invocation)
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(&result, Err(AgentError::ExecutionObservation {error, execution_result})
            if matches!(error.as_ref(), AgentError::Protocol(_))
                && execution_result.as_deref() == Some(&Err(rejection)))
            );
            let saved = storage.snapshot();
            assert_eq!(saved.invocations.len(), 1);
            assert_eq!(saved.invocations[0].events, vec![observed]);
            assert_eq!(saved.invocations[0].result, Some(result));
            assert!(saved.invocations[0].provider_report.is_none());
            assert_eq!(saved.invocations[0].submission, mode);
            if mode == SubmissionMode::Immediate {
                assert!(saved.invocations[0].scheduling.is_empty());
            } else {
                let settlement = saved.invocations[0].scheduling.last().unwrap();
                assert_eq!(settlement.stage, InvocationStage::Settled);
                assert_eq!(
                    settlement.cause,
                    if matches!(update, ExecutionUpdate::Finished(_)) {
                        SchedulingCause::ExecutionSettled
                    } else {
                        SchedulingCause::ExecutionFailed
                    }
                );
                assert!(settlement.actor.is_none());
            }
            assert_eq!(
                *backend.close_requests.lock().unwrap(),
                vec![SessionCloseRequest::ExecutionFailed]
            );
            *backend.late_output.lock().unwrap() = None;
            *backend.execution_error.lock().unwrap() = None;
            backend.execution_rejected.store(false, Ordering::SeqCst);
            backend.suppress_terminal.store(false, Ordering::SeqCst);
            recover_after_automatic_stop(&agent).await;
            assert_eq!(
                agent.invoke(input("after-cleanup"), actor()).await,
                Ok(ExecutionOutcome::Completed)
            );
            assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
            agent.close(actor()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn clean_rejection_does_not_wait_for_an_open_observation_stream() {
    let (agent, backend, storage) = probe(false).await;
    backend.execution_rejected.store(true, Ordering::SeqCst);
    backend.suppress_terminal.store(true, Ordering::SeqCst);
    let rejection = AgentError::InvalidInput("no observations".into());
    *backend.execution_error.lock().unwrap() = Some(rejection.clone());
    // The sender remains alive, so next() is Pending rather than end-of-stream.
    assert_eq!(
        timeout(
            Duration::from_secs(2),
            agent.invoke(input("clean-rejection"), actor())
        )
        .await
        .unwrap(),
        Err(rejection)
    );
    assert_eq!(backend.closes.load(Ordering::SeqCst), 0);
    assert!(storage.snapshot().invocations[0].events.is_empty());
    *backend.execution_error.lock().unwrap() = None;
    backend.execution_rejected.store(false, Ordering::SeqCst);
    backend.suppress_terminal.store(false, Ordering::SeqCst);
    assert_eq!(
        agent.invoke(input("usable"), actor()).await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(backend.closes.load(Ordering::SeqCst), 0);
    agent.close(actor()).await.unwrap();
}
