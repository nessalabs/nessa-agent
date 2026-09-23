//! Cleaned provider generations reject controls until preparation confirms readiness.
use super::*;

async fn assert_permissions_fenced(
    agent: &Agent,
    backend: &Probe,
    calls: usize,
    expected: AgentError,
    stage: &str,
) {
    for operation in [ProviderControl::Answer, ProviderControl::CancelPermission] {
        assert_eq!(
            invoke_control(agent, operation).await,
            Err(expected.clone()),
            "{stage}: {operation:?}"
        );
    }
    assert_eq!(backend.controls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn cleaned_control_generation_fences_permissions_through_gated_restoration() {
    for source in [
        ProviderControl::Answer,
        ProviderControl::CancelPermission,
        ProviderControl::Steer,
    ] {
        let (agent, backend, _) = probe(false).await;
        let (release_active, active_gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(active_gate);
        let active = tokio::spawn({
            let agent = agent.clone();
            async move { agent.invoke(input("active"), actor()).await }
        });
        backend.executing.notified().await;
        backend.preparing.notified().await; // Consume initial preparation notification.
        *backend.control_attachment.lock().unwrap() =
            ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                forced: false,
            }));
        *backend.control_error.lock().unwrap() = Some(AgentError::StalePermission);
        assert_eq!(
            invoke_control(&agent, source).await,
            Err(AgentError::StalePermission)
        );
        let calls = backend.controls.load(Ordering::SeqCst);
        assert_permissions_fenced(
            &agent,
            &backend,
            calls,
            AgentError::AttachmentUnavailable(AttachmentPhase::Absent),
            "after the cleanup-reporting control",
        )
        .await;
        // Steering after cleanup queues for preparation instead of targeting the retired context.
        let (release_prepare, prepare_gate) = oneshot::channel();
        *backend.prepare_gate.lock().unwrap() = Some(prepare_gate);
        let following = agent.steer(input("following"), actor()).await.unwrap();
        let SteeringDelivery::Queued(following) = following else {
            panic!("cleaned context cannot receive native steering");
        };
        assert_eq!(
            backend.steers.load(Ordering::SeqCst),
            usize::from(matches!(source, ProviderControl::Steer))
        );
        release_active.send(()).unwrap();
        assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
        backend.preparing.notified().await;
        assert_permissions_fenced(
            &agent,
            &backend,
            calls,
            AgentError::AttachmentUnavailable(AttachmentPhase::Absent),
            "while replacement preparation is gated",
        )
        .await;
        *backend.control_attachment.lock().unwrap() = ProviderSessionState::Usable;
        release_prepare.send(()).unwrap();
        assert_eq!(following.wait().await, Ok(ExecutionOutcome::Completed));
        assert_eq!(
            invoke_control(&agent, ProviderControl::Answer).await,
            Err(AgentError::StalePermission)
        );
        assert_eq!(backend.controls.load(Ordering::SeqCst), calls + 1);
        agent.close(actor()).await.unwrap();
    }
}

#[tokio::test]
async fn already_pending_permission_cannot_resume_against_a_cleaned_generation() {
    for audit_failure in [false, true] {
        let (agent, backend, _) = probe(false).await;
        let (release, gate) = oneshot::channel();
        *backend.control_gate.lock().unwrap() = Some(gate);
        let waiting = tokio::spawn({
            let agent = agent.clone();
            async move { invoke_control(&agent, ProviderControl::Answer).await }
        });
        backend.control_entered.notified().await;
        let report = if audit_failure {
            cleaned_with_error(AgentError::AuditFailure)
        } else {
            CleanupReport::confirmed(CloseOutcome { forced: false })
        };
        *backend.control_attachment.lock().unwrap() = ProviderSessionState::CleanupReported(report);
        assert!(invoke_control(&agent, ProviderControl::CancelPermission)
            .await
            .is_err());
        *backend.control_attachment.lock().unwrap() = ProviderSessionState::Usable;
        // Both controls were admitted before cleanup; the pending control must
        // recheck provider-generation readiness before its next backend poll.
        assert_eq!(
            timeout(Duration::from_secs(2), waiting)
                .await
                .expect("cleanup wakes already-pending permission controls")
                .unwrap(),
            Err(AgentError::Closed)
        );
        assert!(
            release.send(()).is_err(),
            "stopped control dropped its backend wait"
        );
        assert_permissions_fenced(
            &agent,
            &backend,
            2,
            if audit_failure {
                AgentError::AuditFailure
            } else {
                AgentError::AttachmentUnavailable(AttachmentPhase::Absent)
            },
            "after an already-admitted control is interrupted",
        )
        .await;
        let expected_close = if audit_failure {
            Err(AgentError::AuditFailure)
        } else {
            Ok(CloseOutcome { forced: false })
        };
        assert_eq!(agent.close(actor()).await, expected_close);
    }
}

#[tokio::test]
async fn restoration_cannot_erase_confirmed_cleanup_audit_failure() {
    for source in [
        ProviderControl::Answer,
        ProviderControl::CancelPermission,
        ProviderControl::Steer,
    ] {
        for later_success_report in [false, true] {
            let (agent, backend, storage) = probe(false).await;
            let (release_active, active_gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(active_gate);
            let active = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("active"), actor()).await }
            });
            backend.executing.notified().await;
            *backend.control_attachment.lock().unwrap() =
                ProviderSessionState::CleanupReported(cleaned_with_error(AgentError::AuditFailure));
            *backend.control_error.lock().unwrap() = Some(AgentError::StalePermission);
            let expected = if matches!(source, ProviderControl::Steer) {
                AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(AgentError::StalePermission),
                    cleanup_error: Box::new(AgentError::AuditFailure),
                }
            } else {
                AgentError::StalePermission
            };
            assert_eq!(invoke_control(&agent, source).await, Err(expected.clone()));
            let calls = backend.controls.load(Ordering::SeqCst);
            let admission_error = AgentError::AuditFailure;
            assert_permissions_fenced(
                &agent,
                &backend,
                calls,
                AgentError::AuditFailure,
                "after confirmed cleanup whose audit failed",
            )
            .await;
            // A later successful physical-cleanup report from the already-running
            // execution must not acknowledge the control's earlier failed audit.
            if later_success_report {
                *backend.execution_attachment.lock().unwrap() =
                    ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                        forced: false,
                    }));
            }
            release_active.send(()).unwrap();
            assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
            assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Absent);
            assert!(matches!(
                agent.authorize_attachment(AttachmentRequest::AutomaticRecovery),
                Err(AgentError::Closed)
            ));
            assert!(matches!(
                agent.enqueue(input("blocked-queue"), actor()).await,
                Err(AgentError::AuditFailure)
            ));
            assert!(matches!(
                agent.steer(input("blocked-steering"), actor()).await,
                Err(AgentError::AuditFailure)
            ));
            let (_release_prepare, prepare_gate) = oneshot::channel();
            *backend.prepare_gate.lock().unwrap() = Some(prepare_gate);
            for id in ["next-one", "next-two"] {
                assert_eq!(
                    timeout(Duration::from_secs(2), agent.invoke(input(id), actor()))
                        .await
                        .expect("audit failure must return before gated provider preparation"),
                    Err(AgentError::AuditFailure)
                );
                assert!(
                    backend.prepare_gate.lock().unwrap().is_some(),
                    "failed audit precedes provider preparation"
                );
                assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
                let saved = storage.snapshot();
                let admission = saved
                    .invocations
                    .iter()
                    .find(|record| record.request.execution_id.as_str() == id);
                if matches!(source, ProviderControl::Steer) {
                    // Native delivery finalized failed cleanup before this call:
                    // admission fails before it can create another invocation.
                    assert!(admission.is_none());
                    assert_eq!(
                        saved.invocations.last().unwrap().result,
                        Some(Err(expected.clone()))
                    );
                } else {
                    assert_eq!(
                        admission.unwrap().result,
                        Some(Err(AgentError::AuditFailure))
                    );
                }
                assert_permissions_fenced(
                    &agent,
                    &backend,
                    calls,
                    admission_error.clone(),
                    "after a blocked direct invocation",
                )
                .await;
            }
            assert_eq!(
                backend.steers.load(Ordering::SeqCst),
                usize::from(matches!(source, ProviderControl::Steer))
            );
            for _ in 0..2 {
                assert_eq!(agent.close(actor()).await, Err(AgentError::AuditFailure));
            }
            assert_eq!(backend.closes.load(Ordering::SeqCst), 0);
        }
    }
}
