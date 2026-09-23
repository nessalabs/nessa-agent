use super::support::*;
use serde_json::Value;
use std::fs::read_to_string;
use tokio::{sync::oneshot, time::Instant};

#[tokio::test]
async fn slow_consumer_hits_shared_byte_budget_and_still_audits_and_cleans_up() {
    let _slot = process_test_slot().await;
    for reject_audit in [false, true] {
        let audit = Arc::new(RecordingAudit {
            reject: reject_audit,
            ..Default::default()
        });
        // These individually valid maxima previously allowed about 64 GiB of text.
        let (root, mut config, model) = test_acp_configuration("permission-byte-flood", 4096);
        config.max_frame_bytes = 16 * 1024 * 1024;
        config.max_incoming_frame_bytes = 16 * 1024 * 1024;
        config.execution_timeout = None;
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
        let active = start(&opened, "byte-budget").await;
        assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
        let ExecutionUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
            panic!("expected review before oversized stream");
        };
        // Keep the event consumer alive without reading its large messages. Byte
        // exhaustion, not the 4096-slot count or consumer loss, must end execution.
        // The fixture must decode more than the fixed 32 MiB queue budget to
        // exercise this boundary, which an idle debug build moves through the real
        // child-process transport in about ten seconds per pass. This bound is a
        // deadlock guard, never a throughput assertion, so it is sized far above
        // that cost: a host running the rest of this suite alongside it has been
        // measured at three times the idle figure, and exceeding the bound must
        // mean nothing is moving at all.
        let failure = timeout(Duration::from_secs(180), active)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_gone(&root, "pid");
        if reject_audit {
            assert_eq!(
                failure,
                ordered_failures(&[
                    AgentError::Backpressure,
                    AgentError::AuditFailure,
                    AgentError::AuditFailure,
                    AgentError::AuditFailure,
                ])
            );
            assert_eq!(
                opened
                    .session
                    .shutdown(SessionCloseRequest::Explicit(close_action()))
                    .await
                    .into_result(),
                Err(failure)
            );
        } else {
            assert_eq!(failure, AgentError::Backpressure);
            opened
                .session
                .shutdown(SessionCloseRequest::Explicit(close_action()))
                .await
                .into_result()
                .unwrap();
            let cancellations = audit.records.lock().unwrap();
            assert_eq!(cancellations.len(), 1);
            assert_eq!(cancellations[0].request().id(), &id);
            assert_eq!(
                cancellations[0].request().execution_id().as_str(),
                "byte-budget"
            );
            assert_eq!(cancellations[0].origin(), &CancellationOrigin::Runtime);
            assert_eq!(
                cancellations[0].request().state(),
                PermissionStateView::Cancelled {
                    reason: &PermissionCancellationReason::execution_failed(),
                }
            );
            assert_eq!(audit.closures.lock().unwrap().len(), 1);
            assert_eq!(audit.finishes.lock().unwrap().len(), 1);
        }
        let mut text_bytes = 0;
        while let Ok(Some(event)) = opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error())
        {
            if let ExecutionUpdate::Message(chunk) = event.into_update() {
                assert_eq!(chunk.kind(), MessageKind::Text);
                text_bytes += chunk.payload_bytes();
            }
        }
        assert_eq!(text_bytes, 30 * 1024 * 1024);
    }
}

#[tokio::test]
async fn protocol_failures_and_output_overflow_close_owned_scope() {
    let _process_slot = process_test_slot().await;
    for mode in [
        "malformed",
        "oversize",
        "wrong-session",
        "unknown-reason",
        "config-change",
        "unknown-tool",
        "empty-message-identity",
        "provider-error",
        "eof",
        "flood",
    ] {
        let (root, binding) = test_acp_binding(mode, 2);
        let opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
        let result = timeout(
            Duration::from_secs(3),
            opened.session.execute(prompt("test")),
        )
        .await
        .unwrap()
        .into_result();
        assert!(result.is_err(), "{mode}: {result:?}");
        if mode == "provider-error" {
            assert_eq!(
                result,
                Err(AgentError::Provider {
                    code: -32000,
                    diagnostic: Some(ProviderDiagnostic::new(
                        "provider plan does not allow this request",
                    )),
                })
            );
        }
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn maps_terminal_reasons_and_rejects_client_execution_requests() {
    let _process_slot = process_test_slot().await;
    for (mode, expected) in [
        ("max_tokens", ExecutionOutcome::OutputLimit),
        ("max_turn_requests", ExecutionOutcome::RequestLimit),
        ("refusal", ExecutionOutcome::Refused),
        ("cancelled", ExecutionOutcome::Cancelled),
        ("unknown-request", ExecutionOutcome::Completed),
    ] {
        let (root, binding) = test_acp_binding(mode, 16);
        let opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
        assert_eq!(
            opened
                .session
                .execute(prompt("test"))
                .await
                .into_result()
                .unwrap(),
            expected
        );
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn idle_binding_failures_are_visible_to_the_event_consumer() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("idle-config-change", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    assert!(matches!(
        opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::Protocol(_))
    ));
    assert_eq!(
        opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error())
            .unwrap(),
        None
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let records = audit.closures.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].closure().session_id(), opened.session.id());
    assert_eq!(records[0].closure().execution_id(), None);
    assert_eq!(
        records[0].closure().reason(),
        &PermissionCancellationReason::session_failed()
    );
    assert_eq!(records[0].origin(), &CancellationOrigin::Runtime);
    assert!(audit.finishes.lock().unwrap().is_empty());
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn terminal_delivery_failure_is_reported_by_both_prompt_and_event_reader() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("echo", 1, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let ProviderExecutionReply::Finished(settlement) = opened.session.execute(prompt("full")).await
    else {
        panic!("provider was dispatched")
    };
    assert_eq!(
        settlement.provider_result(),
        Some(&Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(settlement.failure(), Some(&AgentError::Backpressure));
    let expected = settlement.into_result().unwrap_err();
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("full"))
    );
    assert_eq!(
        opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error()),
        Err(expected)
    );
    assert_eq!(
        opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error()),
        Ok(None)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let closures = audit.closures.lock().unwrap();
    assert_eq!(closures.len(), 1);
    assert_eq!(closures[0].closure().execution_id(), None);
    assert_eq!(
        closures[0].closure().reason(),
        &PermissionCancellationReason::session_failed()
    );
    let finishes = audit.finishes.lock().unwrap();
    assert_eq!(finishes.len(), 1);
    assert_eq!(finishes[0].result(), &Ok(ExecutionOutcome::Completed));
    assert_gone(&root, "pid");
}

struct WriteDeadlineAudit {
    reject_closure: bool,
    records: RecordingAudit,
    closure: Mutex<Option<oneshot::Sender<Instant>>>,
}
impl ExecutionAudit for WriteDeadlineAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            let closure = matches!(record, ExecutionAuditRecord::SessionClosed(_));
            if closure {
                if let Some(sender) = self.closure.lock().unwrap().take() {
                    sender.send(Instant::now()).unwrap();
                }
            }
            self.records.record(record).await?;
            if closure && self.reject_closure {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn blocked_prompt_write_obeys_execution_deadline_or_the_default_write_bound() {
    let _slot = process_test_slot().await;
    for (limit, reject_closure) in [
        (Some(Duration::from_millis(20)), false),
        (None, false),
        (Some(Duration::from_millis(20)), true),
    ] {
        let (sender, receiver) = oneshot::channel();
        let audit = Arc::new(WriteDeadlineAudit {
            reject_closure,
            records: RecordingAudit::default(),
            closure: Mutex::new(Some(sender)),
        });
        let (root, mut config, model) = test_acp_configuration("blocked-prompt-write", 16);
        config.execution_timeout = limit;
        config.max_frame_bytes = 4 * 1024 * 1024;
        let binding = ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            audit.clone(),
        )
        .unwrap();
        let opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
        let mut request = prompt("blocked-write");
        request.user_message =
            UserMessage::text_only(PromptText::new("x".repeat(2 * 1024 * 1024)).unwrap());
        tokio::time::pause();
        let began = Instant::now();
        let session = opened.session.clone();
        let running = tokio::spawn(async move { session.execute(request).await.into_result() });
        let closure_time = receiver.await.unwrap();
        tokio::time::resume();
        let result = running.await.unwrap();
        let expected_result = if reject_closure {
            Err(ordered_failures(&[
                AgentError::Deadline,
                AgentError::AuditFailure,
            ]))
        } else {
            Err(AgentError::Deadline)
        };
        assert_eq!(result, expected_result);
        let elapsed = closure_time - began;
        // Without an execution deadline the write has its own bound, which
        // grows with the frame: one second, and one more for each whole
        // mebibyte of this two-mebibyte prompt.
        let expected = limit.unwrap_or(Duration::from_secs(3));
        // Tokio's timer wheel rounds expiry to its next millisecond tick.
        assert!(
            elapsed >= expected && elapsed <= expected + Duration::from_millis(1),
            "{elapsed:?}"
        );
        assert_gone(&root, "pid");
        let records = audit.records.closures.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].closure().session_id(), opened.session.id());
        assert_eq!(
            records[0].closure().execution_id().unwrap().as_str(),
            "blocked-write"
        );
        assert_eq!(
            records[0].closure().reason(),
            &PermissionCancellationReason::deadline_exceeded()
        );
        assert_eq!(records[0].origin(), &CancellationOrigin::Runtime);
        let finishes = audit.records.finishes.lock().unwrap();
        assert_eq!(finishes.len(), 1);
        assert_eq!(
            finishes[0].result(),
            &Err(PermissionCancellationReason::deadline_exceeded())
        );
    }
}

#[tokio::test]
async fn acp_message_id_is_retained_for_each_streamed_fragment() {
    let _slot = process_test_slot().await;
    let (_root, binding) = test_acp_binding("message-identities", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let ProviderExecutionReply::Finished(report) = opened.session.execute(prompt("hello")).await
    else {
        panic!("dispatched")
    };
    report.into_result().unwrap();
    for (id, text) in [("m1", "First "), ("m1", "reply."), ("m2", "Second reply.")] {
        assert_eq!(
            next(&mut opened).await,
            ExecutionUpdate::Message(
                MessageChunk::text(text).with_message_id(MessageId::new(id).unwrap()),
            )
        );
    }
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

#[tokio::test]
async fn an_agent_frame_over_the_inbound_ceiling_fails_although_the_host_writes_larger_ones() {
    let _process_slot = process_test_slot().await;
    // What this host writes and what it will accept are two separate ceilings.
    // Carrying images needs the first to be large; the second is the buffer an
    // agent subprocess can make this host allocate, and stays small.
    let (root, mut config, model) = test_acp_configuration("oversize", 16);
    config.max_frame_bytes = 16 * 1024 * 1024;
    config.max_incoming_frame_bytes = 8192;
    config.execution_timeout = None;
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let mut large = prompt("large");
    let text = "x".repeat(64 * 1024);
    large.user_message = UserMessage::text_only(PromptText::new(&text).unwrap());
    // This message is eight times the inbound ceiling and is admitted and
    // written; the fixture's twenty-thousand-byte answer is what fails.
    let result = timeout(Duration::from_secs(5), opened.session.execute(large))
        .await
        .unwrap()
        .into_result();
    assert!(matches!(result, Err(AgentError::Protocol(_))), "{result:?}");
    let observed: Value =
        serde_json::from_str(&read_to_string(root.path().join("prompt-observed")).unwrap())
            .unwrap();
    assert_eq!(observed[0]["text"].as_str().map(str::len), Some(text.len()));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}
