//! Live responses and completion cancellation retain their operation deadlines.
use super::*;
use crate::application::agent_execution::{
    executions::ExecutionRequest, permissions::ActionContext,
};
use crate::domain::agent_execution::{
    permissions::PermissionStateView,
    prompts::{PromptText, UserMessage},
};
use crate::infrastructure::acp::executions::event_queue::EventReceiver;
use std::{
    fs::File,
    io::{ErrorKind, Write},
    os::fd::AsFd,
    sync::{atomic::AtomicBool, Mutex},
};

#[derive(Default)]
struct Audit {
    records: Mutex<Vec<ExecutionAuditRecord>>,
    reject: AtomicBool,
}
impl ExecutionAudit for Audit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        Box::pin(async {
            if self.reject.load(Ordering::SeqCst) {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}
async fn blocked_worker(
    frames: &str,
    retain_executable_use: bool,
) -> (
    Worker<TestAcpProfile>,
    ExecutionController,
    EventReceiver,
    Option<ProcessCleanup>,
    Arc<ManualClock>,
) {
    let (_root, mut config, capabilities) = profile_setup();
    config.shutdown_grace = Duration::from_millis(20);
    let clock = manual_clock(&mut config);
    let recovery = retain_executable_use.then(|| {
        let executable_use = config.executable.admit().unwrap();
        ProcessCleanup::new(config.clone(), executable_use)
    });
    let mut command = tokio::process::Command::new("/usr/bin/python3");
    command.args([
        "-c",
        "import os,sys,time;os.write(1,b'!'+sys.argv[1].encode());time.sleep(60)",
        frames,
    ]);
    let mut scope = ProcessScope::spawn(command).unwrap();
    let mut stdout = scope.stdout.take().unwrap();
    stdout.read_exact(&mut [0]).await.unwrap();
    let mut filler = File::from(
        scope
            .stdin
            .as_ref()
            .unwrap()
            .as_fd()
            .try_clone_to_owned()
            .unwrap(),
    );
    loop {
        match filler.write(&[b'x'; 4096]) {
            Ok(size) => assert!(size > 0),
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("pipe fill failed: {error}"),
        }
    }
    let (_commands, commands) = mpsc::channel(1);
    let (_close, close_requested) = watch::channel(None);
    let (operation_capabilities, _) = watch::channel(ProviderOperationCapabilities::default());
    let (events, receiver) = EventQueueBudget::new().channel(16);
    let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    let execution_id = ExecutionId::new("active").unwrap();
    execution.begin_execution(execution_id.clone()).unwrap();
    let (reply, _) = oneshot::channel();
    let worker = Worker {
        profile: TestAcpProfile {
            reject_startup: false,
            reject_session: false,
        },
        audit: Arc::new(Audit::default()),
        cancellation_cause: None,
        reader: Reader::new(stdout, config.max_incoming_frame_bytes),
        scope,
        config,
        capabilities,
        commands,
        close_requested,
        events,
        sequence: 0,
        permission_sequence: Arc::new(AtomicU64::new(0)),
        question_sequence: Arc::new(AtomicU64::new(0)),
        active: Some(ActiveExecution {
            id: 1,
            execution_id,
            reply,
            deadline: None,
        }),
        steering: None,
        steering_supported: false,
        agent_accepts_images: false,
        operation_capabilities,
        permissions: HashMap::new(),
        startup_advisory_session: None,
        questions: HashMap::new(),
        declined: None,
        shutdown_deadline: None,
        configured: true,
        closing: false,
        deferred_outcome: None,
        provider_result: None,
        settlement_facts: SettlementFacts::new(),
        correlation_sequence: 0,
        failure_cause: ObservationFailureCause::ExecutionFailed,
    };
    (worker, execution, receiver, recovery, clock)
}

#[tokio::test]
async fn live_nested_responses_observe_earliest_execution_or_steering_deadline() {
    for steering_first in [false, true] {
        for method in ["unsupported/test", "session/request_permission"] {
            let (mut worker, mut execution, _events, _recovery, clock) =
                blocked_worker("", false).await;
            let began = clock.now();
            let soon = began + Duration::from_millis(20);
            let later = began + Duration::from_millis(200);
            worker.active.as_mut().unwrap().deadline =
                Some(if steering_first { later } else { soon });
            let (reply, _) = oneshot::channel();
            worker.steering = Some(PendingSteering {
                id: 2,
                reply,
                deadline: if steering_first { soon } else { later },
            });
            let message = serde_json::from_value(
                json!({"jsonrpc":"2.0","id":77,"method":method,"params":{"sessionId":"context"}}),
            )
            .unwrap();
            worker.config.tools_enabled = false;
            let deadline = worker.current_deadline();
            // The earlier of the two is the one waited for.
            let result = ending_at(
                &clock,
                soon,
                worker.message(&mut execution, message, deadline),
            )
            .await
            .map_err(WorkerFailure::into_error);
            worker
                .scope
                .cleanup(Duration::ZERO, Duration::from_secs(2))
                .await
                .unwrap();
            assert_eq!(result, Err(AgentError::Deadline), "{method}");
            assert_eq!(
                worker.failure_cause,
                ObservationFailureCause::DeadlineExceeded
            );
            assert_eq!(
                worker.cancellation_cause,
                Some((
                    PermissionCancellationReason::deadline_exceeded(),
                    CancellationOrigin::Runtime
                ))
            );
        }
    }
}

#[tokio::test]
async fn completion_permission_timeout_does_not_restart_shutdown_grace() {
    for reject_audit in [false, true] {
        let (mut worker, mut execution, _events, recovery, clock) = blocked_worker("", true).await;
        let recovery = recovery.expect("this fixture admitted executable use before spawn");
        let audit = Arc::new(Audit::default());
        worker.audit = audit.clone();
        worker
            .permission(&mut execution, RpcId::Number(77), permission_params(), None)
            .await
            .map_err(WorkerFailure::into_error)
            .unwrap();
        assert_eq!(worker.permissions.len(), 1);
        let began = clock.now();
        let message = serde_json::from_value(
            json!({"jsonrpc":"2.0","id":1,"result":{"stopReason":"end_turn"}}),
        )
        .unwrap();
        let grace_ends = began + Duration::from_millis(20);
        assert_eq!(
            ending_at(
                &clock,
                grace_ends,
                worker.message(&mut execution, message, None)
            )
            .await
            .map_err(WorkerFailure::into_error),
            Err(AgentError::Deadline)
        );
        assert_eq!(
            worker.provider_result,
            Some(Ok(ExecutionOutcome::Completed))
        );
        assert_eq!(worker.shutdown_deadline, Some(grace_ends));
        // Answered without the clock moving: no second grace interval.
        assert_eq!(
            promptly(worker.send_cancellation("context")).await,
            Ok(()),
            "fallback granted another grace interval"
        );
        audit.reject.store(reject_audit, Ordering::SeqCst);
        let failure = worker.record_failure(OperationEffectPhase::Worker, AgentError::Deadline);
        let completed = worker
            .finish(&mut Some(execution), Err(failure), &mut None, &recovery)
            .await;
        assert!(completed.cleanup.is_confirmed());
        assert_eq!(
            completed.settlement.provider_result(),
            Some(&Ok(ExecutionOutcome::Completed))
        );
        assert_eq!(
            completed.cleanup.operation_failure(),
            Some(&AgentError::Deadline)
        );
        assert_eq!(
            completed.cleanup.audit(),
            &if reject_audit {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        );
        let records = audit.records.lock().unwrap();
        let cancellations: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                ExecutionAuditRecord::Cancelled(value) => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(cancellations.len(), 1);
        assert_eq!(cancellations[0].session_id().as_str(), "context");
        assert_eq!(cancellations[0].request().execution_id().as_str(), "active");
        assert_eq!(cancellations[0].request().tool_id().as_str(), "tool");
        assert_eq!(cancellations[0].origin(), &CancellationOrigin::Runtime);
        assert!(
            matches!(cancellations[0].request().state(), PermissionStateView::Cancelled { reason } if reason == &PermissionCancellationReason::execution_finished())
        );
    }
}

fn permission_params() -> Value {
    json!({"sessionId":"context", "toolCall":{"toolCallId":"tool", "title":"Read", "kind":"read", "status":"pending", "rawInput":{"target":"/tmp/file"}}, "options":[{"optionId":"allow", "name":"Allow", "kind":"allow_once"},{"optionId":"deny", "name":"Deny", "kind":"reject_once"}]})
}

#[tokio::test]
async fn selected_dispatch_deadline_bounds_idle_permission_response() {
    for method in ["unsupported/test", "session/request_permission"] {
        let frames = format!(
            "{}\n",
            json!({"jsonrpc":"2.0","id":77,"method":method,"params":{"sessionId":"context"}})
        );
        let (mut worker, _, _events, _recovery, clock) = blocked_worker(&frames, false).await;
        worker.active = None;
        let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
        let (reply, _result) = oneshot::channel();
        let began = clock.now();
        let dispatch_ends = began + Duration::from_millis(20);
        let result = ending_at(
            &clock,
            dispatch_ends,
            worker.drain_ready_before_dispatch(
                &mut execution,
                &DispatchCaller::Execution(&reply),
                Some(dispatch_ends),
            ),
        )
        .await;
        worker
            .scope
            .cleanup(Duration::ZERO, Duration::from_secs(2))
            .await
            .unwrap();
        assert!(
            matches!(
                result.map_err(WorkerFailure::into_error),
                Err(AgentError::Deadline)
            ),
            "{method}"
        );
        assert!(execution.active_execution_id().is_none());
        assert_eq!(
            worker.failure_cause,
            ObservationFailureCause::DeadlineExceeded
        );
    }
}

#[tokio::test]
async fn successful_completion_releases_its_temporary_shutdown_deadline() {
    let (mut worker, mut execution, _events, _recovery, clock) = blocked_worker("", false).await;
    let message =
        serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"result":{"stopReason":"end_turn"}}))
            .unwrap();
    worker
        .message(&mut execution, message, None)
        .await
        .map_err(WorkerFailure::into_error)
        .unwrap();
    assert_eq!(worker.shutdown_deadline, None);
    assert!(worker.active.is_none());
    // A later turn must use its own budget, not the completed turn's grace.
    let id = ExecutionId::new("following").unwrap();
    execution.begin_execution(id.clone()).unwrap();
    let (reply, _) = oneshot::channel();
    let began = clock.now();
    let deadline = began + Duration::from_millis(60);
    worker.active = Some(ActiveExecution {
        id: 2,
        execution_id: id,
        reply,
        deadline: Some(deadline),
    });
    let result = ending_at(
        &clock,
        deadline,
        worker.send(json_rpc::unsupported(&RpcId::Number(88))),
    )
    .await;
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(result, Err(AgentError::Deadline));
}

#[tokio::test]
async fn shutdown_grace_preserves_explicit_cause_past_old_steering_deadline() {
    let (mut worker, mut execution, _events, _recovery, clock) = blocked_worker("", false).await;
    let actor = ActionContext::new("owner", "phone", "close").unwrap();
    let request = SessionCloseRequest::Explicit(actor);
    let cause = (request.reason(), request.origin());
    worker.closing = true;
    worker.cancellation_cause = Some(cause.clone());
    let began = clock.now();
    worker.begin_shutdown_grace();
    let (reply, _) = oneshot::channel();
    worker.steering = Some(PendingSteering {
        id: 2,
        reply,
        deadline: began + Duration::from_millis(5),
    });
    // The grace, not the older steering deadline, is what the loop waits for.
    let result = ending_at(
        &clock,
        began + Duration::from_millis(20),
        worker.drive(&mut execution),
    )
    .await;
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    assert!(result.is_ok());
    assert_eq!(worker.cancellation_cause, Some(cause));
}

fn request(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 1,
    }
}

#[tokio::test]
async fn a_steering_acknowledgement_is_armed_with_what_the_read_left() {
    let (mut worker, mut execution, _events, _recovery, clock) = blocked_worker("", false).await;
    worker.steering_supported = true;
    let began = clock.now();
    // Most of the steering deadline went on reading this message's images
    // before its command reached the worker, and this is the rest of it.
    let remaining = Duration::from_millis(300);
    assert!(remaining < steering::RESPONSE_TIMEOUT);
    let (reply, _outcome) = oneshot::channel();
    let command = Command::Steer(
        ExecutionId::new("active").unwrap(),
        dispatched(request("steer"), Some(began + remaining)),
        reply,
    );
    let result = ending_at(
        &clock,
        began + remaining,
        worker.command(&mut execution, command),
    )
    .await
    .map_err(WorkerFailure::into_error);
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(result, Err(AgentError::Deadline));
    assert_eq!(
        worker.steering.as_ref().map(|pending| pending.deadline),
        Some(began + remaining),
        "a fresh steering interval was armed"
    );
}

#[tokio::test]
async fn an_execution_write_is_armed_with_what_the_read_left() {
    let (mut worker, _, _events, _recovery, clock) = blocked_worker("", false).await;
    worker.active = None;
    let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    // The configured limit is longer than what is left, so arming from it
    // would give this prompt more time than its caller was promised.
    let configured = worker.config.execution_timeout.expect("a configured limit");
    let remaining = Duration::from_millis(300);
    assert!(remaining < configured);
    let began = clock.now();
    let (reply, _result) = oneshot::channel();
    let command =
        Command::ExecutionRequest(dispatched(request("next"), Some(began + remaining)), reply);
    let result = ending_at(
        &clock,
        began + remaining,
        worker.command(&mut execution, command),
    )
    .await
    .map_err(WorkerFailure::into_error);
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(result, Err(AgentError::Deadline));
    assert_eq!(
        worker.active.as_ref().and_then(|active| active.deadline),
        Some(began + remaining),
        "a fresh execution interval was armed"
    );
}

fn ask_params(field: &str) -> Value {
    json!({"sessionId":"context","mode":"form","message":"Which environment?",
        "requestedSchema":{"type":"object","properties":{
            field:{"type":"string","oneOf":[{"const":"staging"}]}}}})
}

/// A sink that is still acknowledging each record when it is first polled.
///
/// Within its own bound every time, so each record it is given is delivered;
/// what it exposes is anything that shares one deadline across several of them
/// — a deadline already reached drops whatever is pending under it.
#[derive(Default)]
struct SlowAudit {
    records: Mutex<Vec<ExecutionAuditRecord>>,
}
impl ExecutionAudit for SlowAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        Box::pin(async {
            tokio::task::yield_now().await;
            Ok(())
        })
    }
}

/// A teardown's deadline ends what the provider is told, not what is recorded.
///
/// Review found the asks taken out, then told, closed and audited one by one
/// under the grace deadline: an audit sink that answered within its own bound,
/// but slower than the grace left, had the rest of the loop dropped with the
/// future — every ask not yet reached lost its closure and its record. Here the
/// provider never reads, so no cancellation can be sent, and each ask must still
/// close and be recorded as ended, with its delivery reported as failed.
#[tokio::test]
async fn asks_ended_by_a_teardown_are_closed_and_recorded_even_when_its_deadline_expires() {
    let (mut worker, mut execution, mut events, _recovery, clock) = blocked_worker("", false).await;
    let audit = Arc::new(SlowAudit::default());
    worker.audit = audit.clone();
    for (wire, field) in [(71, "question_0"), (72, "question_1"), (73, "question_2")] {
        worker
            .question(&mut execution, RpcId::Number(wire), ask_params(field), None)
            .await
            .map_err(WorkerFailure::into_error)
            .unwrap();
    }
    assert_eq!(worker.questions.len(), 3);
    // The provider never reads, so every cancellation waits for the grace to
    // run out, and the teardown ends exactly there.
    let deadline = worker.begin_shutdown_grace();
    assert_eq!(
        ending_at(&clock, deadline, worker.send_cancellation("context")).await,
        Ok(())
    );
    assert!(worker.questions.is_empty());

    let mut asked = Vec::new();
    let mut closed = Vec::new();
    for _ in 0..6 {
        let event = timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("each ask and each close is observed")
            .unwrap();
        match event.update() {
            ExecutionUpdate::QuestionAsked { id, .. } => asked.push(id.clone()),
            ExecutionUpdate::QuestionClosed { id } => closed.push(id.clone()),
            _ => {}
        }
    }
    closed.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    assert_eq!(asked.len(), 3);
    assert_eq!(
        closed, asked,
        "every ask closes, not only those reached in time"
    );

    let records = audit.records.lock().unwrap();
    let ended: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::QuestionAnswered(value) => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(ended.len(), 3, "every ask's ending is recorded");
    for record in ended {
        assert_eq!(
            record.response(),
            &QuestionResponse::Cancelled(QuestionCancellation::SessionEnded)
        );
        assert_eq!(record.actor(), None);
        assert_eq!(record.execution_id().as_str(), "active");
        assert_eq!(
            record.delivery(),
            &PermissionAnswerDelivery::Failed(AgentError::Deadline)
        );
    }
}

/// A review that cannot be cancelled does not decide whether an ask ends.
///
/// Review cancellations stop at the first failed write. Asks were ended after
/// them, so a provider that had gone away with a review and an ask both pending
/// left the ask open, unclosed and unrecorded.
#[tokio::test]
async fn an_ask_ends_with_evidence_when_a_pending_review_cannot_be_cancelled() {
    let (mut worker, mut execution, mut events, _recovery, _clock) =
        blocked_worker("", false).await;
    let audit = Arc::new(Audit::default());
    worker.audit = audit.clone();
    worker
        .permission(&mut execution, RpcId::Number(77), permission_params(), None)
        .await
        .map_err(WorkerFailure::into_error)
        .unwrap();
    worker
        .question(
            &mut execution,
            RpcId::Number(78),
            ask_params("question_0"),
            None,
        )
        .await
        .map_err(WorkerFailure::into_error)
        .unwrap();
    // The provider is gone: nothing can be written to it at all.
    worker.scope.stdin = None;
    assert_eq!(
        promptly(worker.send_cancellation("context")).await,
        Err(AgentError::Closed)
    );
    assert!(worker.questions.is_empty());
    let mut closed = false;
    for _ in 0..3 {
        let event = timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("the review, the ask and its close are observed")
            .unwrap();
        closed |= matches!(event.update(), ExecutionUpdate::QuestionClosed { .. });
    }
    assert!(closed, "the ask closes");
    let records = audit.records.lock().unwrap();
    let ended: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::QuestionAnswered(value) => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(ended.len(), 1, "{records:?}");
    assert_eq!(
        ended[0].delivery(),
        &PermissionAnswerDelivery::Failed(AgentError::Closed)
    );
}

/// An answer whose decision cannot be recorded leaves its ask to the teardown.
///
/// Review found the ask released before the decision was audited: when the
/// sink refused, nothing was sent, and the teardown that followed found no ask
/// to end — no closure, no record, and an agent never told.
#[tokio::test]
async fn an_ask_whose_answer_could_not_be_recorded_is_still_ended_with_evidence() {
    let (mut worker, mut execution, mut events, _recovery, clock) = blocked_worker("", false).await;
    let audit = Arc::new(Audit::default());
    worker.audit = audit.clone();
    worker
        .question(
            &mut execution,
            RpcId::Number(79),
            ask_params("question_0"),
            None,
        )
        .await
        .map_err(WorkerFailure::into_error)
        .unwrap();
    let asked = timeout(Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    let ExecutionUpdate::QuestionAsked { id, .. } = asked.update().clone() else {
        panic!("expected the ask");
    };
    audit.reject.store(true, Ordering::SeqCst);
    let (reply, replied) = oneshot::channel();
    let answered = worker
        .answer_question(
            &execution,
            QuestionAnswer {
                actor: ActionContext::new("person", "panel", "answer").unwrap(),
                execution_id: ExecutionId::new("active").unwrap(),
                id: id.clone(),
                choices: None,
            },
            reply,
        )
        .await
        .map_err(WorkerFailure::into_error);
    assert_eq!(answered, Err(AgentError::AuditFailure));
    assert!(replied.await.unwrap().is_err());
    assert_eq!(
        worker.questions.len(),
        1,
        "an unrecorded answer ends nothing"
    );

    audit.reject.store(false, Ordering::SeqCst);
    let deadline = worker.begin_shutdown_grace();
    assert_eq!(
        ending_at(&clock, deadline, worker.send_cancellation("context")).await,
        Ok(())
    );
    let closed = timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("the ask closes")
        .unwrap();
    assert!(
        matches!(closed.update(), ExecutionUpdate::QuestionClosed { id: closed } if closed == &id)
    );
    let records = audit.records.lock().unwrap();
    let ended: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::QuestionAnswered(value) => Some(value),
            _ => None,
        })
        .collect();
    // The refused decision was offered to the sink, and the ending after it
    // is recorded with its own cause and no initiator invented.
    assert_eq!(ended.len(), 2, "{records:?}");
    assert_eq!(ended[0].response(), &QuestionResponse::Declined);
    assert_eq!(
        ended[1].response(),
        &QuestionResponse::Cancelled(QuestionCancellation::SessionEnded)
    );
    assert_eq!(ended[1].actor(), None);
}
