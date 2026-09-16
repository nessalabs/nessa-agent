//! Ready wire policy evidence precedes prompt writes, including native steering.
use super::*;
use crate::application::agent_execution::{executions::ExecutionRequest, tools::ToolReviewInput};
use crate::domain::agent_execution::{prompts::PromptText, tools::ToolCallUpdate};
use crate::infrastructure::acp::executions::event_queue::EventReceiver;
use std::sync::Mutex;

struct PolicyProfile {
    inner: TestAcpProfile,
    updates: AtomicU64,
    close_after: Option<(u64, watch::Sender<Option<SessionCloseRequest>>)>,
    drop_reply_after: Option<(
        u64,
        Mutex<Option<oneshot::Receiver<ProviderExecutionReply>>>,
    )>,
}
impl AcpProfile for PolicyProfile {
    fn validate_initialize(&self, value: &Value) -> Result<(), AgentError> {
        self.inner.validate_initialize(value)
    }
    fn new_session_params(&self, config: &AcpConfig, caps: &EffectiveCapabilities) -> Value {
        self.inner.new_session_params(config, caps)
    }
    fn session_configuration(&self, id: &str) -> Option<Value> {
        self.inner.session_configuration(id)
    }
    fn verify_session(
        &self,
        value: &Value,
        caps: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        self.inner.verify_session(value, caps, configured)
    }
    fn verify_update(
        &self,
        kind: &str,
        value: &Value,
        _: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        let count = self.updates.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((threshold, close)) = &self.close_after {
            if count == *threshold {
                close.send_replace(Some(SessionCloseRequest::SessionHandlesDropped));
            }
        }
        if let Some((threshold, reply)) = &self.drop_reply_after {
            if count == *threshold {
                drop(reply.lock().unwrap().take());
            }
        }
        match kind {
            "current_mode_update" if value["currentModeId"] != "default" => {
                Err(json_rpc::protocol("permission mode changed"))
            }
            "config_option_update" if value["model"] != "expected" => {
                Err(json_rpc::protocol("model changed"))
            }
            _ => Ok(()),
        }
    }
    fn validate_execution(
        &self,
        input: &ExecutionRequest,
        caps: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        self.inner.validate_execution(input, caps)
    }
    fn begin_execution(&mut self) {
        self.inner.begin_execution();
    }
    fn tool_call(&mut self, value: &Value) -> Result<ToolCallUpdate, AgentError> {
        self.inner.tool_call(value)
    }
    fn tool_input(&self, value: &Value) -> Result<ToolReviewInput, AgentError> {
        self.inner.tool_input(value)
    }
}
fn request() -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new("next").unwrap(),
        user_message: PromptText::new("read file").unwrap(),
        estimated_input_tokens: 1,
        reserved_output_tokens: 10,
    }
}
fn notification(update: Value) -> Value {
    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"context","update":update}})
}
async fn worker_with_ready_frames(
    frames: &[Value],
    prefix: &str,
) -> (
    Worker<PolicyProfile>,
    mpsc::Sender<Command>,
    watch::Sender<Option<SessionCloseRequest>>,
    EventReceiver,
) {
    worker_with_ready_frames_boundary(frames, prefix, false).await
}
async fn worker_with_flushed_ready_frames(
    frames: &[Value],
    prefix: &str,
) -> (
    Worker<PolicyProfile>,
    mpsc::Sender<Command>,
    watch::Sender<Option<SessionCloseRequest>>,
    EventReceiver,
) {
    worker_with_ready_frames_boundary(frames, prefix, true).await
}
async fn worker_with_ready_frames_boundary(
    frames: &[Value],
    prefix: &str,
    await_flush: bool,
) -> (
    Worker<PolicyProfile>,
    mpsc::Sender<Command>,
    watch::Sender<Option<SessionCloseRequest>>,
    EventReceiver,
) {
    let (_, mut config, capabilities) = profile_setup();
    config.max_frame_bytes = 16 * 1024;
    let mut bytes = serde_json::to_string(&json!({"jsonrpc":"2.0","method":"ready"})).unwrap();
    bytes.push('\n');
    bytes.push_str(prefix);
    for frame in frames {
        bytes.push_str(&serde_json::to_string(frame).unwrap());
        bytes.push('\n');
    }
    let mut process = tokio::process::Command::new("/usr/bin/python3");
    let marker = await_flush.then(|| tempfile::tempdir().unwrap());
    let marker_path = marker.as_ref().map(|marker| marker.path().join("flushed"));
    if let Some(marker_path) = &marker_path {
        process.args(["-c", "import sys,json;sys.stdout.buffer.write(sys.argv[1].encode());sys.stdout.buffer.flush();open(sys.argv[2],'w').close();m=json.loads(sys.stdin.readline());print(json.dumps({'jsonrpc':'2.0','id':m['id'],'error':{'code':-32099,'message':'test prompt observed'}}),flush=True)", &bytes, marker_path.to_str().unwrap()]);
    } else {
        process.args(["-c", "import sys,json;sys.stdout.buffer.write(sys.argv[1].encode());sys.stdout.buffer.flush();m=json.loads(sys.stdin.readline());print(json.dumps({'jsonrpc':'2.0','id':m['id'],'error':{'code':-32099,'message':'test prompt observed'}}),flush=True)", &bytes]);
    }
    let mut scope = ProcessScope::spawn(process).unwrap();
    let mut reader = Reader::new(scope.stdout.take().unwrap(), config.max_frame_bytes);
    assert_eq!(
        reader.next().await.unwrap().method.as_deref(),
        Some("ready")
    );
    if let Some(marker_path) = marker_path {
        while !marker_path.exists() {
            tokio::task::yield_now().await;
        }
    }
    let (sender, commands) = mpsc::channel(4);
    let (close, close_requested) = watch::channel(None);
    let (operation_capabilities, _) = watch::channel(OperationCapabilities::default());
    let (events, receiver) = EventQueueBudget::new().channel(16);
    (
        Worker {
            profile: PolicyProfile {
                inner: TestAcpProfile {
                    reject_startup: false,
                    reject_session: false,
                },
                updates: AtomicU64::new(0),
                close_after: None,
                drop_reply_after: None,
            },
            audit: Arc::new(UnexpectedAudit),
            cancellation_cause: None,
            scope,
            reader,
            config,
            capabilities,
            commands,
            close_requested,
            events,
            sequence: 0,
            permission_sequence: Arc::new(AtomicU64::new(0)),
            active: None,
            steering: None,
            steering_supported: true,
            operation_capabilities,
            permissions: HashMap::new(),
            shutdown_deadline: None,
            closing: false,
            deferred_outcome: None,
            provider_result: None,
            audit_failure: None,
            failure_cause: ObservationFailureCause::ExecutionFailed,
        },
        sender,
        close,
        receiver,
    )
}
#[derive(Debug, Clone, Copy)]
enum Dispatch {
    Prompt,
    Steering,
}

#[tokio::test]
async fn same_poll_policy_drift_prevents_prompt_and_native_steering_writes() {
    for dispatch in [Dispatch::Prompt, Dispatch::Steering] {
        for (update, reason) in [
            (
                json!({"sessionUpdate":"current_mode_update","currentModeId":"bypassPermissions"}),
                "permission mode changed",
            ),
            (
                json!({"sessionUpdate":"config_option_update","model":"changed"}),
                "model changed",
            ),
        ] {
            for prefix in ["", "\n\n"] {
                let (mut worker, commands, _close, _events) =
                    worker_with_flushed_ready_frames(&[notification(update.clone())], prefix).await;
                let mut execution =
                    ExecutionController::new(ExecutionSessionId::new("context").unwrap());
                let (reply, result) = oneshot::channel();
                let (steer_reply, steer_result) = oneshot::channel();
                if matches!(dispatch, Dispatch::Steering) {
                    execution
                        .begin_execution(ExecutionId::new("active").unwrap())
                        .unwrap();
                    worker.active = Some(ActiveExecution {
                        id: 99,
                        execution_id: ExecutionId::new("active").unwrap(),
                        reply,
                        deadline: None,
                    });
                    commands
                        .send(Command::Steer(
                            ExecutionId::new("active").unwrap(),
                            request(),
                            steer_reply,
                        ))
                        .await
                        .unwrap();
                } else {
                    commands
                        .send(Command::ExecutionRequest(request(), reply))
                        .await
                        .unwrap();
                }
                let failure = worker.drive(&mut execution).await;
                worker
                    .scope
                    .cleanup(Duration::ZERO, Duration::from_secs(2))
                    .await
                    .unwrap();
                assert_eq!(
                    failure,
                    Err(json_rpc::protocol(reason)),
                    "{dispatch:?}, prefix={prefix:?}"
                );
                assert_eq!(
                    worker.sequence, 0,
                    "no prompt ID allocated before validation"
                );
                assert!(worker.steering.is_none());
                if matches!(dispatch, Dispatch::Prompt) {
                    assert!(worker.active.is_none());
                    let ProviderExecutionReply::Finished(report) = result.await.unwrap() else {
                        panic!("fatal policy failure must fence attachment");
                    };
                    assert_eq!(report.provider_result(), None);
                    assert_eq!(
                        report.session_state(),
                        &ProviderSessionState::CleanupRequired
                    );
                    assert_eq!(report.into_result(), Err(json_rpc::protocol(reason)));
                } else {
                    let failure = steer_result.await.unwrap().unwrap_err();
                    assert_eq!(failure.error(), &json_rpc::protocol(reason));
                    assert_eq!(
                        failure.session_state(),
                        &ProviderSessionState::CleanupRequired
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn valid_ready_burst_larger_than_batch_preserves_prompt_dispatch() {
    let frames = vec![notification(json!({"sessionUpdate":"usage_update"})); 64];
    let (mut worker, commands, _close, _events) = worker_with_ready_frames(&frames, "").await;
    let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    let (reply, _result) = oneshot::channel();
    commands
        .send(Command::ExecutionRequest(request(), reply))
        .await
        .unwrap();
    let failure = worker.drive(&mut execution).await;
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(failure, Err(AgentError::Provider { code: -32099 }));
    assert_eq!(worker.sequence, 1);
    assert_eq!(
        execution.active_execution_id(),
        Some(&ExecutionId::new("next").unwrap())
    );
}

struct CloseAudit {
    reject: bool,
    records: Mutex<Vec<ExecutionAuditRecord>>,
}
impl ExecutionAudit for CloseAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            self.records.lock().unwrap().push(record);
            if self.reject {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}
#[tokio::test]
async fn close_interrupts_ready_policy_backlog_without_dispatching_pending_prompt() {
    for reject_audit in [false, true] {
        // The complete ready sequence fits one pipe write and reader buffer. The helper
        // consumes only `ready`, leaving the policy frame readable before the
        // command is admitted; no scheduler timing or sleep establishes the race.
        let frames = vec![
            notification(
                json!({"sessionUpdate":"current_mode_update","currentModeId":"default"})
            );
            1
        ];
        let (mut worker, commands, close, _events) =
            worker_with_flushed_ready_frames(&frames, "").await;
        worker.profile.close_after = Some((1, close));
        let audit = Arc::new(CloseAudit {
            reject: reject_audit,
            records: Mutex::new(Vec::new()),
        });
        worker.audit = audit.clone();
        let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
        let (reply, result) = oneshot::channel();
        commands
            .send(Command::ExecutionRequest(request(), reply))
            .await
            .unwrap();
        let failure = worker.drive(&mut execution).await;
        worker
            .scope
            .cleanup(Duration::ZERO, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(
            failure,
            if reject_audit {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        );
        assert!(matches!(
            result.await.unwrap(),
            ProviderExecutionReply::Rejected(AgentError::Closed)
        ));
        assert_eq!(worker.sequence, 0);
        assert_eq!(worker.profile.updates.load(Ordering::SeqCst), 1);
        assert_eq!(
            worker.cancellation_cause,
            Some((
                SessionCloseRequest::SessionHandlesDropped.reason(),
                CancellationOrigin::Runtime
            ))
        );
        assert_eq!(audit.records.lock().unwrap().len(), 1);
        assert!(execution.is_closed());
    }
}

#[tokio::test]
async fn exhausted_task_budget_does_not_hide_ready_policy_frames() {
    let update = notification(
        json!({"sessionUpdate":"current_mode_update","currentModeId":"bypassPermissions"}),
    );
    let (mut worker, _commands, _close, _events) =
        worker_with_flushed_ready_frames(&[update], "").await;
    let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    let (reply, result) = oneshot::channel();
    while tokio::task::coop::has_budget_remaining() {
        tokio::task::consume_budget().await;
    }
    let failure = worker
        .command(&mut execution, Command::ExecutionRequest(request(), reply))
        .await;
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(failure, Err(json_rpc::protocol("permission mode changed")));
    assert_eq!(worker.sequence, 0);
    let ProviderExecutionReply::Finished(report) = result.await.unwrap() else {
        panic!("fatal policy failure must fence attachment");
    };
    assert_eq!(report.provider_result(), None);
    assert_eq!(
        report.session_state(),
        &ProviderSessionState::CleanupRequired
    );
}

#[tokio::test]
async fn selected_operation_deadline_includes_ready_policy_validation() {
    // Repetition catches reactor-notification lag: every provider write is
    // flushed before command admission, and none may be mistaken for quiescence.
    for dispatch in [Dispatch::Prompt, Dispatch::Steering]
        .into_iter()
        .cycle()
        .take(32)
    {
        let frames = Vec::new();
        // The whitespace prefix starts an incomplete transport frame. Deadline
        // advancement after the first poll deterministically wins before dispatch.
        let (mut worker, commands, _close, _events) =
            worker_with_flushed_ready_frames(&frames, &" ".repeat(500)).await;
        let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
        let (reply, result) = oneshot::channel();
        let (steer_reply, steer_result) = oneshot::channel();
        let limit = if matches!(dispatch, Dispatch::Prompt) {
            worker.config.execution_timeout = Some(Duration::from_secs(1));
            commands
                .send(Command::ExecutionRequest(request(), reply))
                .await
                .unwrap();
            Duration::from_secs(1)
        } else {
            execution
                .begin_execution(ExecutionId::new("active").unwrap())
                .unwrap();
            worker.active = Some(ActiveExecution {
                id: 99,
                execution_id: ExecutionId::new("active").unwrap(),
                reply,
                deadline: None,
            });
            commands
                .send(Command::Steer(
                    ExecutionId::new("active").unwrap(),
                    request(),
                    steer_reply,
                ))
                .await
                .unwrap();
            steering::RESPONSE_TIMEOUT
        };
        tokio::time::pause();
        let failure = {
            let drive = worker.drive(&mut execution);
            tokio::pin!(drive);
            poll_fn(|cx| {
                assert!(drive.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            tokio::time::advance(limit).await;
            drive.await
        };
        tokio::time::resume();
        worker
            .scope
            .cleanup(Duration::ZERO, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(failure, Err(AgentError::Deadline), "{dispatch:?}");
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
        assert_eq!(worker.sequence, 0, "{dispatch:?}");
        if matches!(dispatch, Dispatch::Prompt) {
            let ProviderExecutionReply::Finished(report) = result.await.unwrap() else {
                panic!("deadline must fence attachment");
            };
            assert_eq!(report.provider_result(), None);
            assert_eq!(
                report.session_state(),
                &ProviderSessionState::CleanupRequired
            );
            assert_eq!(report.into_result(), Err(AgentError::Deadline));
        } else {
            let failure = steer_result.await.unwrap().unwrap_err();
            assert_eq!(failure.error(), &AgentError::Deadline);
            assert_eq!(
                failure.session_state(),
                &ProviderSessionState::CleanupRequired
            );
        }
    }
}

#[tokio::test]
async fn ready_completion_keeps_native_steering_prompt_fallback() {
    let response = json!({"jsonrpc":"2.0","id":99,"result":{"stopReason":"end_turn"}});
    let (mut worker, _commands, _close, mut events) =
        worker_with_ready_frames(&[response], "").await;
    worker.audit = Arc::new(CloseAudit {
        reject: false,
        records: Mutex::new(Vec::new()),
    });
    let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    execution
        .begin_execution(ExecutionId::new("active").unwrap())
        .unwrap();
    let (active_reply, completed) = oneshot::channel();
    worker.active = Some(ActiveExecution {
        id: 99,
        execution_id: ExecutionId::new("active").unwrap(),
        reply: active_reply,
        deadline: None,
    });
    let (reply, result) = oneshot::channel();
    let operation = worker
        .command(
            &mut execution,
            Command::Steer(ExecutionId::new("active").unwrap(), request(), reply),
        )
        .await;
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(operation, Ok(()));
    assert_eq!(
        result.await.unwrap().unwrap(),
        SteeringOutcome::PromptRequired
    );
    assert!(
        matches!(completed.await.unwrap(), ProviderExecutionReply::Finished(report) if report.provider_result() == Some(&Ok(ExecutionOutcome::Completed)))
    );
    assert_eq!(
        events.recv().await.unwrap().update(),
        &ExecutionUpdate::Finished(ExecutionOutcome::Completed)
    );
    assert_eq!(worker.sequence, 0);
    assert!(worker.active.is_none());
}

#[tokio::test]
async fn interrupted_dispatch_receipt_retains_deadline_and_consumer_failure() {
    for deadline in [false, true] {
        let (mut worker, _commands, _close, events) = worker_with_ready_frames(&[], "").await;
        let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
        let mut events = Some(events);
        if deadline {
            execution
                .begin_execution(ExecutionId::new("active").unwrap())
                .unwrap();
            let (reply, _result) = oneshot::channel();
            worker.active = Some(ActiveExecution {
                id: 99,
                execution_id: ExecutionId::new("active").unwrap(),
                reply,
                deadline: Some(Instant::now()),
            });
        } else {
            drop(events.take());
        }
        let (reply, result) = oneshot::channel();
        assert_eq!(
            worker
                .command(
                    &mut execution,
                    Command::Steer(ExecutionId::new("active").unwrap(), request(), reply)
                )
                .await,
            Ok(())
        );
        let failure = result.await.unwrap().unwrap_err();
        assert_eq!(
            failure.error(),
            &if deadline {
                AgentError::Deadline
            } else {
                AgentError::Backpressure
            }
        );
        assert_eq!(
            failure.session_state(),
            &ProviderSessionState::CleanupRequired
        );
        assert_eq!(worker.sequence, 0);
        assert_eq!(
            worker.drive(&mut execution).await,
            Err(if deadline {
                AgentError::Deadline
            } else {
                AgentError::Backpressure
            })
        );
        worker
            .scope
            .cleanup(Duration::ZERO, Duration::from_secs(2))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn dropped_selected_caller_leaves_context_available_for_next_request() {
    let frames =
        vec![
            notification(json!({"sessionUpdate":"current_mode_update","currentModeId":"default"}));
            3
        ];
    let (mut worker, _commands, _close, _events) =
        worker_with_flushed_ready_frames(&frames, "").await;
    let mut execution = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    let (reply, result) = oneshot::channel();
    worker.profile.drop_reply_after = Some((3, Mutex::new(Some(result))));
    assert_eq!(
        worker
            .command(&mut execution, Command::ExecutionRequest(request(), reply))
            .await,
        Ok(())
    );
    assert_eq!(worker.profile.updates.load(Ordering::SeqCst), 3);
    assert_eq!(worker.sequence, 0);
    assert!(worker.active.is_none());
    assert!(!worker.closing);
    assert_eq!(worker.cancellation_cause, None);
    let (reply, _result) = oneshot::channel();
    assert_eq!(
        worker
            .command(&mut execution, Command::ExecutionRequest(request(), reply))
            .await,
        Ok(())
    );
    assert_eq!(worker.sequence, 1);
    worker
        .scope
        .cleanup(Duration::ZERO, Duration::from_secs(2))
        .await
        .unwrap();
}
