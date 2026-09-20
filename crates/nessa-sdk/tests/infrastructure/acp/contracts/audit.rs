//! Session closure evidence survives idle state, failure, and missing observers.
use super::support::*;
use crate::application::agent_execution::providers::ObservationFailureCause;
use tokio::sync::oneshot;

#[tokio::test]
async fn explicit_close_audits_idle_and_active_contexts_once() {
    let _slot = process_test_slot().await;
    for running in [false, true] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, binding) = test_acp_binding_with_audit("stall", 16, audit.clone());
        let mut opened = binding.open(None).await.unwrap();
        let active = if running {
            let active = start(&opened, "run").await;
            next(&mut opened).await;
            Some(active)
        } else {
            None
        };
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        if let Some(active) = active {
            assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
        }
        let records = audit.closures.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].closure().session_id(), opened.session.id());
        assert_eq!(
            records[0].closure().execution_id().map(ExecutionId::as_str),
            running.then_some("run")
        );
        assert_eq!(
            records[0].closure().reason(),
            &PermissionCancellationReason::session_closed()
        );
        assert_eq!(
            records[0].origin(),
            &CancellationOrigin::Client(close_action())
        );
        assert!(audit.records.lock().unwrap().is_empty());
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn idle_close_audit_failure_is_visible_after_process_cleanup() {
    let _slot = process_test_slot().await;
    for stall in [false, true] {
        let audit = Arc::new(RecordingAudit {
            reject: !stall,
            stall,
            ..Default::default()
        });
        let (root, binding) = test_acp_binding_with_audit("echo", 16, audit);
        let opened = binding.open(None).await.unwrap();
        assert_eq!(
            timeout(
                Duration::from_secs(3),
                opened
                    .session
                    .shutdown(SessionCloseRequest::Explicit(close_action()))
            )
            .await
            .unwrap()
            .into_result(),
            Err(AgentError::AuditFailure)
        );
        assert_eq!(
            opened
                .session
                .shutdown(SessionCloseRequest::Explicit(close_action()))
                .await
                .into_result(),
            Err(AgentError::AuditFailure)
        );
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn execution_deadline_audits_context_and_execution_cause() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("stall", 16, audit.clone());
    let mut opened = binding.open(None).await.unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("deadline"))
            .await
            .into_result(),
        Err(AgentError::Deadline)
    );
    let observation_failure = loop {
        match opened.events.next().await {
            Ok(Some(_)) => continue,
            Ok(None) => panic!("deadline must remain observable"),
            Err(failure) => break failure,
        }
    };
    assert_eq!(
        observation_failure.cause(),
        ObservationFailureCause::DeadlineExceeded
    );
    assert_eq!(observation_failure.error(), &AgentError::Deadline);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let records = audit.closures.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].closure().session_id(), opened.session.id());
    assert_eq!(
        records[0].closure().execution_id().unwrap().as_str(),
        "deadline"
    );
    assert_eq!(
        records[0].closure().reason(),
        &PermissionCancellationReason::deadline_exceeded()
    );
    assert_eq!(records[0].origin(), &CancellationOrigin::Runtime);
    let finishes = audit.finishes.lock().unwrap();
    assert_eq!(finishes.len(), 1);
    assert_eq!(
        finishes[0].result(),
        &Err(PermissionCancellationReason::deadline_exceeded())
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn event_consumer_loss_cannot_discard_session_closure() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("stall", 16, audit.clone());
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "unobserved").await;
    next(&mut opened).await;
    drop(opened.events);
    assert_eq!(active.await.unwrap(), Err(AgentError::Backpressure));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let records = audit.closures.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].closure().execution_id().unwrap().as_str(),
        "unobserved"
    );
    assert_eq!(
        records[0].closure().reason(),
        &PermissionCancellationReason::event_consumer_dropped()
    );
    assert_eq!(records[0].origin(), &CancellationOrigin::Runtime);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn dropping_idle_session_handles_audits_before_event_stream_ends() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, config, model) = test_acp_configuration("echo", 16);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let mut opened = binding.open(None).await.unwrap();
    let id = opened.session.id().clone();
    drop(opened.session);
    assert_eq!(
        timeout(Duration::from_secs(3), opened.events.next())
            .await
            .unwrap()
            .unwrap(),
        None
    );
    let records = audit.closures.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].closure().session_id(), &id);
    assert_eq!(records[0].closure().execution_id(), None);
    assert_eq!(
        records[0].closure().reason(),
        &PermissionCancellationReason::session_handles_dropped()
    );
    assert_eq!(records[0].origin(), &CancellationOrigin::Runtime);
    assert_gone(&root, "pid");
}

struct RejectClosureAudit {
    records: Mutex<Vec<ExecutionAuditRecord>>,
}
impl ExecutionAudit for RejectClosureAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            let reject = matches!(record, ExecutionAuditRecord::SessionClosed(_));
            self.records.lock().unwrap().push(record);
            if reject {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn failed_session_audit_still_attempts_pending_permission_evidence() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RejectClosureAudit {
        records: Mutex::new(Vec::new()),
    });
    let (root, config, model) = test_acp_configuration("permission-stop", 16);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    let ExecutionUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
        panic!("expected permission")
    };
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result(),
        Err(AgentError::AuditFailure)
    );
    assert_eq!(active.await.unwrap(), Err(AgentError::AuditFailure));
    let records = audit.records.lock().unwrap();
    assert_eq!(records.len(), 3);
    assert!(matches!(records[2], ExecutionAuditRecord::Finished(_)));
    let ExecutionAuditRecord::SessionClosed(closure) = &records[0] else {
        panic!("expected closure")
    };
    let ExecutionAuditRecord::Cancelled(permission) = &records[1] else {
        panic!("expected cancellation")
    };
    assert_eq!(permission.request().id(), &id);
    assert_eq!(permission.session_id(), closure.closure().session_id());
    assert_eq!(
        Some(permission.request().execution_id()),
        closure.closure().execution_id()
    );
    assert_eq!(permission.origin(), closure.origin());
    assert_eq!(
        permission.request().state(),
        PermissionStateView::Cancelled {
            reason: &closure.closure().reason().clone()
        }
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn every_permission_free_outcome_has_once_only_execution_evidence() {
    let _slot = process_test_slot().await;
    for (mode, expected) in [
        ("echo", Ok(ExecutionOutcome::Completed)),
        ("max_tokens", Ok(ExecutionOutcome::OutputLimit)),
        ("cancelled", Ok(ExecutionOutcome::Cancelled)),
        (
            "provider-error",
            Err(PermissionCancellationReason::execution_failed()),
        ),
    ] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, mut config, model) = test_acp_configuration(mode, 16);
        // This test asserts audit semantics for completed provider outcomes. A
        // short execution deadline turns scheduler contention into an unrelated
        // deadline outcome when the real fixture process is under load.
        config.execution_timeout = Some(Duration::from_secs(30));
        let binding = ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            audit.clone(),
        )
        .unwrap();
        let opened = binding.open(None).await.unwrap();
        let session = opened.session.id().clone();
        let result = opened.session.execute(prompt("test")).await.into_result();
        assert_eq!(result.is_ok(), expected.is_ok());
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        let records = audit.finishes.lock().unwrap();
        assert_eq!(records.len(), 1, "{mode}");
        assert_eq!(records[0].session_id(), &session);
        assert_eq!(
            records[0].execution_id(),
            &ExecutionId::new("test").unwrap()
        );
        assert_eq!(records[0].result(), &expected);
        if mode == "provider-error" {
            let closures = audit.closures.lock().unwrap();
            assert_eq!(closures.len(), 1);
            assert_eq!(
                closures[0].closure().execution_id(),
                Some(records[0].execution_id())
            );
            assert_eq!(
                closures[0].closure().reason(),
                &PermissionCancellationReason::execution_failed()
            );
        }
        assert_gone(&root, "pid");
    }
}

struct RejectFinishAudit {
    records: Mutex<Vec<ExecutionAuditRecord>>,
}
impl ExecutionAudit for RejectFinishAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            let reject = matches!(record, ExecutionAuditRecord::Finished(_));
            self.records.lock().unwrap().push(record);
            if reject {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn failed_execution_audit_prevents_success_and_still_cleans_up() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RejectFinishAudit {
        records: Mutex::new(Vec::new()),
    });
    let (root, config, model) = test_acp_configuration("echo", 16);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let mut opened = binding.open(None).await.unwrap();
    let ProviderExecutionReply::Finished(settlement) = opened.session.execute(prompt("test")).await
    else {
        panic!("provider was dispatched")
    };
    assert_eq!(
        settlement.provider_result(),
        Some(&Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(settlement.failure(), Some(&AgentError::AuditFailure));
    let ProviderSessionState::CleanupReported(cleanup) = settlement.session_state() else {
        panic!("cleanup report required")
    };
    assert!(cleanup.is_confirmed());
    assert_eq!(cleanup.audit(), &Err(AgentError::AuditFailure));
    let expected = settlement.into_result().unwrap_err();
    loop {
        match opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error())
        {
            Ok(Some(event)) => assert!(!matches!(event.update(), ExecutionUpdate::Finished(_))),
            Err(error) => {
                assert_eq!(error, expected);
                break;
            }
            Ok(None) => break,
        }
    }
    let records = audit.records.lock().unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, ExecutionAuditRecord::Finished(_)))
            .count(),
        1
    );
    let closure = records
        .iter()
        .find_map(|record| match record {
            ExecutionAuditRecord::SessionClosed(record) => Some(record),
            _ => None,
        })
        .expect("settled context still needs closure evidence");
    assert_eq!(closure.closure().execution_id(), None);
    assert_eq!(
        closure.closure().reason(),
        &PermissionCancellationReason::session_failed()
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn malformed_provider_control_fails_unlimited_execution_and_audits_cleanup() {
    let _slot = process_test_slot().await;
    for mode in [
        "permission-missing-id",
        "permission-null-id",
        "duplicate-json-mode",
    ] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, mut config, model) = test_acp_configuration(mode, 16);
        config.execution_timeout = None;
        let binding = ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            audit.clone(),
        )
        .unwrap();
        let opened = binding.open(None).await.unwrap();
        let result = timeout(
            Duration::from_secs(3),
            opened.session.execute(prompt("invalid-control")),
        )
        .await
        .unwrap()
        .into_result();
        assert!(
            matches!(result, Err(AgentError::Protocol(_))),
            "{mode}: {result:?}"
        );
        assert_gone(&root, "pid");
        let records = audit.closures.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].closure().session_id(), opened.session.id());
        assert_eq!(
            records[0].closure().execution_id().unwrap().as_str(),
            "invalid-control"
        );
        assert_eq!(
            records[0].closure().reason(),
            &PermissionCancellationReason::execution_failed()
        );
        assert_eq!(records[0].origin(), &CancellationOrigin::Runtime);
        assert!(audit.records.lock().unwrap().is_empty());
    }
}

struct FirstCancellationStalls {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    attempted: Mutex<Vec<PermissionCancellation>>,
    delivered: Mutex<Vec<PermissionCancellation>>,
}
impl ExecutionAudit for FirstCancellationStalls {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if let ExecutionAuditRecord::Cancelled(record) = record {
                let first = {
                    let mut attempted = self.attempted.lock().unwrap();
                    attempted.push(record.clone());
                    attempted.len() == 1
                };
                if first {
                    self.entered
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(())
                        .unwrap();
                    return std::future::pending().await;
                }
                // Durable delivery needs another timer poll; an already-expired
                // shared deadline cannot complete it even though this is prompt.
                tokio::time::sleep(Duration::from_millis(1)).await;
                self.delivered.lock().unwrap().push(record);
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn each_bulk_cancellation_gets_its_own_audit_deadline() {
    let _slot = process_test_slot().await;
    let (entered, first_started) = oneshot::channel();
    let audit = Arc::new(FirstCancellationStalls {
        entered: Mutex::new(Some(entered)),
        attempted: Mutex::new(Vec::new()),
        delivered: Mutex::new(Vec::new()),
    });
    let (root, mut config, model) = test_acp_configuration("permission-pair", 16);
    config.execution_timeout = None;
    let audit_deadline = config.shutdown_grace;
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "bulk").await;
    next(&mut opened).await;
    let mut ids = Vec::new();
    for _ in 0..2 {
        let ExecutionUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
            panic!("expected review")
        };
        ids.push(id);
    }
    tokio::time::pause();
    let closing = tokio::spawn({
        let session = opened.session.clone();
        async move {
            session
                .shutdown(SessionCloseRequest::Explicit(close_action()))
                .await
                .into_result()
        }
    });
    first_started.await.unwrap();
    // Advance only the stalled audit call. Leaving time paused while closing a
    // real child can expire all cleanup stages before the OS schedules/reaps it.
    tokio::time::advance(audit_deadline + Duration::from_millis(1)).await;
    tokio::time::resume();
    assert_eq!(closing.await.unwrap(), Err(AgentError::AuditFailure));
    assert_eq!(active.await.unwrap(), Err(AgentError::AuditFailure));
    assert_gone(&root, "pid");
    let attempted = audit.attempted.lock().unwrap();
    let delivered = audit.delivered.lock().unwrap();
    assert_eq!(attempted.len(), 2);
    assert_eq!(delivered.as_slice(), &attempted[1..]);
    for record in attempted.iter() {
        assert!(ids.contains(record.request().id()));
        assert_eq!(record.session_id(), opened.session.id());
        assert_eq!(record.request().execution_id().as_str(), "bulk");
        assert_eq!(record.origin(), &CancellationOrigin::Client(close_action()));
        assert_eq!(
            record.request().state(),
            PermissionStateView::Cancelled {
                reason: &PermissionCancellationReason::session_closed()
            }
        );
    }
}

#[tokio::test]
async fn malformed_startup_permission_closes_the_known_idle_context() {
    let _slot = process_test_slot().await;
    for mode in [
        "startup-permission-missing-id",
        "startup-permission-null-id",
    ] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, mut config, model) = test_acp_configuration(mode, 16);
        // The malformed frame is the behavior under test. Leave enough startup
        // time for the real fixture process to run even on a contended builder.
        config.launch_timeout = Duration::from_secs(30);
        config.startup_timeout = Duration::from_secs(30);
        let binding = ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            audit.clone(),
        )
        .unwrap();
        let failure = binding.open(None).await.err().unwrap();
        assert!(matches!(failure.cause(), AgentError::Protocol(_)));
        assert!(failure.cleanup().is_none());
        assert_gone(&root, "pid");
        let records = audit.closures.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].closure().execution_id(), None);
        assert_eq!(
            records[0].closure().reason(),
            &PermissionCancellationReason::session_failed()
        );
        assert_eq!(records[0].origin(), &CancellationOrigin::Runtime);
        assert!(audit.records.lock().unwrap().is_empty());
    }
}
