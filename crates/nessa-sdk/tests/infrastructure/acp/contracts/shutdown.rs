use super::support::*;

#[tokio::test]
async fn runtime_failure_close_uses_the_authoritative_execution_correlation() {
    let _slot = process_test_slot().await;
    for state in ["idle", "completed", "active"] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, binding) = test_acp_binding_with_audit(
            if state == "active" { "stall" } else { "echo" },
            16,
            audit.clone(),
        );
        let mut opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
        let active = if state == "active" {
            let active = start(&opened, "execution").await;
            assert!(matches!(
                next(&mut opened).await,
                ExecutionUpdate::Message(_)
            ));
            Some(active)
        } else {
            if state == "completed" {
                assert_eq!(
                    opened
                        .session
                        .execute(prompt("execution"))
                        .await
                        .into_result(),
                    Ok(ExecutionOutcome::Completed)
                );
            }
            None
        };
        opened
            .session
            .shutdown(SessionCloseRequest::ExecutionFailed)
            .await
            .into_result()
            .unwrap();
        if let Some(active) = active {
            assert_eq!(
                active.await.unwrap(),
                Err(AgentError::ExecutionObservation {
                    error: Box::new(AgentError::Closed),
                    execution_result: Some(Box::new(Ok(ExecutionOutcome::Cancelled)))
                })
            );
        }
        opened
            .session
            .shutdown(SessionCloseRequest::ExecutionFailed)
            .await
            .into_result()
            .unwrap();
        let closures = audit.closures.lock().unwrap();
        assert_eq!(closures.len(), 1, "{state}");
        let closure = &closures[0];
        assert_eq!(closure.origin(), &CancellationOrigin::Runtime);
        if state == "active" {
            assert_eq!(
                closure.closure().execution_id().unwrap().as_str(),
                "execution"
            );
            assert_eq!(
                closure.closure().reason(),
                &PermissionCancellationReason::execution_failed()
            );
        } else {
            assert_eq!(closure.closure().execution_id(), None);
            assert_eq!(
                closure.closure().reason(),
                &PermissionCancellationReason::session_failed()
            );
        }
        let finishes = audit.finishes.lock().unwrap();
        assert_eq!(finishes.len(), usize::from(state != "idle"));
        if state == "completed" {
            assert_eq!(finishes[0].result(), &Ok(ExecutionOutcome::Completed));
        }
        if state == "active" {
            assert_eq!(finishes[0].result(), &Ok(ExecutionOutcome::Cancelled));
            assert_eq!(finishes[0].execution_id().as_str(), "execution");
        }
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn isolated_bindings_and_close_during_streaming() {
    let _process_slot = process_test_slot().await;
    let (root_a, a) = test_acp_binding("stall", 16);
    let (root_b, b) = test_acp_binding("echo", 16);
    let mut a = a
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let mut b = b
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&a, "long").await;
    next(&mut a).await;
    assert_eq!(
        a.session.execute(prompt("busy")).await.into_result(),
        Err(AgentError::Busy)
    );
    a.session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Cancelled);
    assert_gone(&root_a, "pid");
    let active = start(&b, "unaffected").await;
    assert_eq!(
        next(&mut b).await,
        ExecutionUpdate::Message(MessageChunk::text("unaffected"))
    );
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    b.session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root_b, "pid");
}

#[tokio::test]
async fn prompt_deadline_and_dropped_handles_cleanup() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("stall", 16);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    assert_eq!(
        opened.session.execute(prompt("test")).await.into_result(),
        Err(AgentError::Deadline)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
    let (root, binding) = test_acp_binding("echo", 16);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    drop(opened);
    wait_until_gone(&root, "pid").await;
}

#[tokio::test]
async fn force_closes_a_term_resistant_parent_and_reaps_its_child() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("ignore-stop", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "long").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running"))
    );
    let cleanup = timeout(
        Duration::from_secs(3),
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action())),
    )
    .await
    .unwrap()
    .into_result()
    .unwrap();
    assert!(cleanup.forced);
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Cancelled);
    assert_gone(&root, "pid");
    assert_gone(&root, "child-pid");
}

#[tokio::test]
async fn known_completion_wins_a_later_close_without_rewriting_the_result() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("echo", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "done").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("done"))
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn cancelling_open_cleans_up_a_process_that_never_initializes() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("startup-stall", 16);
    let opening = tokio::spawn(async move {
        binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
    });
    wait_for_file(&root, "pid").await;
    opening.abort();
    let _ = opening.await;
    wait_until_gone(&root, "pid").await;
}

#[tokio::test]
async fn dropping_only_the_event_reader_closes_unobservable_execution() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("stall", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "long").await;
    next(&mut opened).await;
    drop(opened.events);
    assert_eq!(active.await.unwrap(), Err(AgentError::Backpressure));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn unlimited_prompt_survives_a_day_and_still_accepts_close() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("stall", 16);
    config.execution_timeout = None;
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "long-running").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running"))
    );
    // Only advance time after real process startup, so virtual startup deadlines
    // cannot race the operating system launching the fixture.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(24 * 60 * 60)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!root.path().join("cancel-observed").exists());
    assert!(!active.is_finished());
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Cancelled);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn dropping_session_handles_records_the_cause_without_a_client_actor() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("permission-stop", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    let ExecutionUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
        panic!("expected permission")
    };
    active.abort();
    assert!(active.await.unwrap_err().is_cancelled());
    drop(opened.session);
    let cancellation = timeout(Duration::from_secs(3), opened.events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ExecutionUpdate::PermissionCancelled(cancellation) = cancellation.into_update() else {
        panic!("expected cancellation")
    };
    assert_eq!(cancellation.request().id(), &id);
    assert_eq!(cancellation.origin(), &CancellationOrigin::Runtime);
    assert_eq!(
        cancellation.request().state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::session_handles_dropped()
        }
    );
    assert_eq!(audit.records.lock().unwrap().as_slice(), &[cancellation]);
    while matches!(
        timeout(Duration::from_secs(3), opened.events.next())
            .await
            .unwrap(),
        Ok(Some(_))
    ) {}
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn late_session_updates_during_close_do_not_change_output_or_cancelled_settlement() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("late-tool-close", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "closing-with-inflight-tool").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running"))
    );
    let cleanup = timeout(
        Duration::from_secs(3),
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action())),
    )
    .await
    .unwrap()
    .into_result();
    let result = timeout(Duration::from_secs(3), active)
        .await
        .unwrap()
        .unwrap();
    assert_gone(&root, "pid");
    assert_eq!(result, Ok(ExecutionOutcome::Cancelled));
    cleanup.unwrap();
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)
    );
    assert_eq!(
        opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error()),
        Ok(None)
    );
    let finishes = audit.finishes.lock().unwrap();
    assert_eq!(finishes.len(), 1);
    assert_eq!(finishes[0].result(), &Ok(ExecutionOutcome::Cancelled));
    let closures = audit.closures.lock().unwrap();
    assert_eq!(closures.len(), 1);
    assert_eq!(
        closures[0].origin(),
        &CancellationOrigin::Client(close_action())
    );
    assert_eq!(
        closures[0].closure().reason(),
        &PermissionCancellationReason::session_closed()
    );
}

#[tokio::test]
async fn requested_failure_shutdown_preserves_typed_error_and_exact_finish_cause() {
    let _slot = process_test_slot().await;
    for (request, reason, expected) in [
        (
            SessionCloseRequest::ExecutionFailed,
            PermissionCancellationReason::execution_failed(),
            AgentError::Closed,
        ),
        (
            SessionCloseRequest::SessionFailed,
            PermissionCancellationReason::session_failed(),
            AgentError::Closed,
        ),
        (
            SessionCloseRequest::DeadlineExceeded,
            PermissionCancellationReason::deadline_exceeded(),
            AgentError::Deadline,
        ),
    ] {
        for reject in [false, true] {
            let audit = Arc::new(RecordingAudit {
                reject,
                ..Default::default()
            });
            let (root, binding) = test_acp_binding_with_audit("stall", 16, audit.clone());
            let mut opened = binding
                .open(ProviderOpenRequest::without_startup_control(None))
                .await
                .unwrap();
            let active = start(&opened, "execution").await;
            next(&mut opened).await;
            let close = opened.session.shutdown(request.clone()).await.into_result();
            let result = active.await.unwrap();
            if reject {
                assert_eq!(
                    result,
                    Err(ordered_failures(&[
                        AgentError::AuditFailure,
                        expected.clone(),
                        AgentError::AuditFailure,
                    ]))
                );
                assert!(close.is_err());
            } else {
                close.unwrap();
                assert_eq!(
                    result,
                    Err(AgentError::ExecutionObservation {
                        error: Box::new(expected.clone()),
                        execution_result: Some(Box::new(Ok(ExecutionOutcome::Cancelled)))
                    })
                );
                let closures = audit.closures.lock().unwrap();
                assert_eq!(closures.len(), 1);
                assert_eq!(closures[0].closure().reason(), &reason);
                assert_eq!(
                    closures[0].closure().execution_id().unwrap().as_str(),
                    "execution"
                );
                assert_eq!(closures[0].origin(), &CancellationOrigin::Runtime);
                let finishes = audit.finishes.lock().unwrap();
                assert_eq!(finishes.len(), 1);
                assert_eq!(finishes[0].result(), &Ok(ExecutionOutcome::Cancelled));
                assert_eq!(finishes[0].execution_id().as_str(), "execution");
            }
            assert_gone(&root, "pid");
        }
    }
}

struct FinishAudit {
    accepted: RecordingAudit,
    attempts: Mutex<Vec<ExecutionFinish>>,
    reject_finish: bool,
}
impl ExecutionAudit for FinishAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if let ExecutionAuditRecord::Finished(finish) = &record {
                self.attempts.lock().unwrap().push(finish.clone());
                if self.reject_finish {
                    return Err(AgentError::Transport("finish audit rejected".into()));
                }
            }
            self.accepted.record(record).await
        })
    }
}

#[tokio::test]
async fn consumer_loss_after_explicit_close_retains_both_causes_and_finishes_once() {
    let _slot = process_test_slot().await;
    for reject_finish in [false, true] {
        let audit = Arc::new(FinishAudit {
            accepted: RecordingAudit::default(),
            attempts: Mutex::new(Vec::new()),
            reject_finish,
        });
        let (root, mut config, model) = test_acp_configuration("consumer-loss-during-close", 16);
        config.execution_timeout = None;
        // A generous grace makes the protocol marker, rather than the timer,
        // determine when the consumer-loss boundary is exercised.
        config.shutdown_grace = Duration::from_secs(30);
        let binding = ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            audit.clone(),
        )
        .unwrap();
        let mut opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
        let context = opened.session.id().clone();
        let active = start(&opened, "independent-reader-loss").await;
        assert!(matches!(
            next(&mut opened).await,
            ExecutionUpdate::Message(_)
        ));
        let session = opened.session.clone();
        let closing = tokio::spawn(async move {
            session
                .shutdown(SessionCloseRequest::Explicit(close_action()))
                .await
                .into_result()
        });
        // The fixture deliberately leaves cancellation unanswered. This marker
        // proves explicit closure and its audit preceded the lost event consumer.
        wait_for_file(&root, "cancel-observed").await;
        assert!(!closing.is_finished());
        drop(opened.events);
        let result = timeout(Duration::from_secs(3), active)
            .await
            .unwrap()
            .unwrap();
        let cleanup = timeout(Duration::from_secs(3), closing)
            .await
            .unwrap()
            .unwrap();
        if reject_finish {
            assert_eq!(
                result,
                Err(AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(AgentError::Backpressure),
                    cleanup_error: Box::new(AgentError::AuditFailure),
                })
            );
            assert_eq!(
                cleanup,
                Err(ordered_failures(&[
                    AgentError::Backpressure,
                    AgentError::AuditFailure,
                ]))
            );
        } else {
            assert_eq!(result, Err(AgentError::Backpressure));
            cleanup.unwrap();
        }
        let _ = opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result();
        let closures = audit.accepted.closures.lock().unwrap();
        assert_eq!(closures.len(), 1);
        assert_eq!(closures[0].closure().session_id(), &context);
        assert_eq!(
            closures[0].closure().execution_id(),
            Some(&prompt("independent-reader-loss").execution_id)
        );
        assert_eq!(
            closures[0].closure().reason(),
            &PermissionCancellationReason::session_closed()
        );
        assert_eq!(
            closures[0].origin(),
            &CancellationOrigin::Client(close_action())
        );
        let attempts = audit.attempts.lock().unwrap();
        assert_eq!(
            attempts.len(),
            1,
            "execution release must not vanish or be repeated"
        );
        assert_eq!(attempts[0].session_id(), &context);
        assert_eq!(
            attempts[0].execution_id(),
            &prompt("independent-reader-loss").execution_id
        );
        assert_eq!(
            attempts[0].result(),
            &Err(PermissionCancellationReason::event_consumer_dropped())
        );
        assert_eq!(
            audit.accepted.finishes.lock().unwrap().len(),
            usize::from(!reject_finish)
        );
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn execution_failure_close_then_provider_completion_preserves_original_closure() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("complete-on-stop", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "execution").await;
    assert!(matches!(
        next(&mut opened).await,
        ExecutionUpdate::Message(_)
    ));
    let cleanup = opened
        .session
        .shutdown(SessionCloseRequest::ExecutionFailed)
        .await;
    assert_eq!(cleanup.operation_failure(), Some(&AgentError::Closed));
    cleanup.into_result().unwrap();
    assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
    let closures = audit.closures.lock().unwrap();
    assert_eq!(closures.len(), 1);
    assert_eq!(
        closures[0].closure().execution_id().unwrap().as_str(),
        "execution"
    );
    assert_eq!(
        closures[0].closure().reason(),
        &PermissionCancellationReason::execution_failed()
    );
    assert_eq!(closures[0].origin(), &CancellationOrigin::Runtime);
    assert_gone(&root, "pid");
}
