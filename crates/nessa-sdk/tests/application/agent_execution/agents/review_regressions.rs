//! Provider-substitution regressions for review-discovered lifecycle boundaries.
mod admission_error_limits;
mod bulk_cancellation_panics;
mod cleanup_audit;
mod direct_supervision;
mod native_queue_state;
mod native_stop;
mod native_storage_panics;
mod provider_restoration;
mod ready_steering;
mod rejection_observations;
mod retained_error_limits;
mod scheduled_panics;
mod settlement_causes;
mod terminal_settlement;
mod undispatched_cancellation;

use super::*;
use nessa_sdk::application::agent_execution::providers::ProviderOpenFuture;
use nessa_sdk::domain::agent_execution::executions::{InvocationStage, SchedulingCause};
use nessa_sdk::domain::agent_execution::ExecutionError;
use std::{
    future::{poll_fn, Future},
    sync::atomic::AtomicBool,
    task::Poll,
};
use tokio::{sync::Notify, time::timeout};

struct Probe {
    fail_close: AtomicBool,
    cleanup_report: Mutex<Option<CleanupReport>>,
    contradictory: bool,
    suppress_terminal: AtomicBool,
    duplicate_terminal: AtomicBool,
    early_output: Mutex<Option<ExecutionUpdate>>,
    late_output: Mutex<Option<ExecutionUpdate>>,
    execution_error: Mutex<Option<AgentError>>,
    execution_attachment: Mutex<ProviderSessionState>,
    execution_rejected: AtomicBool,
    control_error: Mutex<Option<AgentError>>,
    control_attachment: Mutex<ProviderSessionState>,
    control_gate: Mutex<Option<oneshot::Receiver<()>>>,
    control_entered: Notify,
    control_finished: Notify,
    controls: AtomicUsize,
    closes: AtomicUsize,
    close_requests: Mutex<Vec<SessionCloseRequest>>,
    executing: Notify,
    execution_gate: Mutex<Option<oneshot::Receiver<()>>>,
    settled: Notify,
    closing: Notify,
    close_gate: Mutex<Option<oneshot::Receiver<()>>>,
    preparing: Notify,
    prepare_gate: Mutex<Option<oneshot::Receiver<()>>>,
    sender: Mutex<mpsc::UnboundedSender<ExecutionEvent>>,
    executions: AtomicUsize,
    steers: AtomicUsize,
}
struct ProbeFactory {
    backend: Arc<Probe>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<ExecutionEvent>>>,
}
impl AgentProvider for ProbeFactory {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("review", "test", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            let receiver = self.receiver.lock().unwrap().take().unwrap_or_else(|| {
                let (sender, receiver) = mpsc::unbounded_channel();
                *self.backend.sender.lock().unwrap() = sender;
                receiver
            });
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("review").unwrap(),
                    self.backend.clone(),
                    capabilities(),
                ),
                events: Box::new(TestEvents(receiver)),
            })
        })
    }
}
impl Probe {
    async fn control_result(&self) -> Option<AgentError> {
        self.controls.fetch_add(1, Ordering::SeqCst);
        let gate = self.control_gate.lock().unwrap().take();
        self.control_entered.notify_one();
        if let Some(gate) = gate {
            gate.await.unwrap();
        }
        let error = self.control_error.lock().unwrap().clone();
        // No await follows this notification before the Agent observes the result.
        self.control_finished.notify_one();
        error
    }
}
impl ProviderSessionBackend for Probe {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move {
            let result: Result<(), AgentError> = {
                async {
                    let wait = self.prepare_gate.lock().unwrap().take();
                    self.preparing.notify_one();
                    if let Some(wait) = wait {
                        wait.await.map_err(|_| AgentError::Closed)?;
                    }
                    Ok(())
                }
            }
            .await;
            result
                .map_err(|error| ProviderOperationFailure::new(error, ProviderSessionState::Usable))
        })
    }
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        // Each operation retains the output generation current when the provider
        // accepted it. A later recovery cannot redirect delayed old output into
        // the replacement attachment's stream.
        let sender = self.sender.lock().unwrap().clone();
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> = {
                async move {
                    self.executions.fetch_add(1, Ordering::SeqCst);
                    self.executing.notify_one();
                    if let Some(update) = self.early_output.lock().unwrap().take() {
                        sender
                            .send(ExecutionEvent::new(input.execution_id.clone(), update))
                            .unwrap();
                    }
                    if !self.suppress_terminal.load(Ordering::SeqCst) {
                        sender
                            .send(ExecutionEvent::new(
                                input.execution_id.clone(),
                                ExecutionUpdate::Finished(if self.contradictory {
                                    ExecutionOutcome::Cancelled
                                } else {
                                    ExecutionOutcome::Completed
                                }),
                            ))
                            .unwrap();
                    }
                    if self.duplicate_terminal.load(Ordering::SeqCst) {
                        sender
                            .send(ExecutionEvent::new(
                                input.execution_id.clone(),
                                ExecutionUpdate::Finished(ExecutionOutcome::Completed),
                            ))
                            .unwrap();
                    }
                    if let Some(update) = self.late_output.lock().unwrap().clone() {
                        sender
                            .send(ExecutionEvent::new(input.execution_id.clone(), update))
                            .unwrap();
                    }
                    let gate = self.execution_gate.lock().unwrap().take();
                    if let Some(gate) = gate {
                        gate.await.unwrap();
                    }
                    self.settled.notify_one();
                    match self.execution_error.lock().unwrap().clone() {
                        Some(error) => Err(error),
                        None => Ok(ExecutionOutcome::Completed),
                    }
                }
            }
            .await;
            if self.execution_rejected.load(Ordering::SeqCst) {
                ProviderExecutionReply::Rejected(result.expect_err("explicit admission rejection"))
            } else {
                ProviderExecutionReply::Finished(ExecutionReport::new(
                    Some(result),
                    None,
                    self.execution_attachment.lock().unwrap().clone(),
                ))
            }
        })
    }
    fn steer(
        &self,
        _: ExecutionId,
        _: ExecutionRequest,
    ) -> ProviderOperationFuture<'_, SteeringOutcome> {
        Box::pin(async move {
            let result: Result<SteeringOutcome, AgentError> = {
                self.steers.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    match self.control_result().await {
                        Some(error) => Err(error),
                        None => Ok(SteeringOutcome::Injected),
                    }
                })
            }
            .await;
            result.map_err(|error| {
                ProviderOperationFailure::new(
                    error,
                    self.control_attachment.lock().unwrap().clone(),
                )
            })
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async move {
            let result: Result<PermissionResolution, AgentError> = {
                async {
                    Err(self
                        .control_result()
                        .await
                        .unwrap_or(AgentError::StalePermission))
                }
            }
            .await;
            result.map_err(|error| {
                ProviderOperationFailure::new(
                    error,
                    self.control_attachment.lock().unwrap().clone(),
                )
            })
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async move {
            let result: Result<PermissionCancellation, AgentError> = {
                async {
                    Err(self
                        .control_result()
                        .await
                        .unwrap_or(AgentError::StalePermission))
                }
            }
            .await;
            result.map_err(|error| {
                ProviderOperationFailure::new(
                    error,
                    self.control_attachment.lock().unwrap().clone(),
                )
            })
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.closes.fetch_add(1, Ordering::SeqCst);
            self.close_requests.lock().unwrap().push(request);
            let gate = self.close_gate.lock().unwrap().take();
            self.closing.notify_one();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            if let Some(report) = self.cleanup_report.lock().unwrap().clone() {
                report
            } else if self.fail_close.load(Ordering::SeqCst) {
                CleanupReport::unconfirmed(AgentError::CleanupUncertain)
            } else {
                CleanupReport::confirmed(CloseOutcome { forced: false })
            }
        })
    }
}
async fn probe(contradictory: bool) -> (Agent, Arc<Probe>, MemoryStorage) {
    let storage = MemoryStorage::default();
    let (agent, backend) = probe_with_manager(contradictory, storage.manager().await).await;
    (agent, backend, storage)
}

async fn probe_with_manager(contradictory: bool, manager: SessionManager) -> (Agent, Arc<Probe>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let backend = Arc::new(Probe {
        fail_close: AtomicBool::new(false),
        cleanup_report: Mutex::new(None),
        contradictory,
        suppress_terminal: AtomicBool::new(false),
        duplicate_terminal: AtomicBool::new(false),
        early_output: Mutex::new(None),
        late_output: Mutex::new(None),
        execution_error: Mutex::new(None),
        execution_attachment: Mutex::new(ProviderSessionState::Usable),
        execution_rejected: AtomicBool::new(false),
        control_error: Mutex::new(None),
        control_attachment: Mutex::new(ProviderSessionState::Usable),
        control_gate: Mutex::new(None),
        control_entered: Notify::new(),
        control_finished: Notify::new(),
        controls: AtomicUsize::new(0),
        closes: AtomicUsize::new(0),
        close_requests: Mutex::new(Vec::new()),
        executing: Notify::new(),
        execution_gate: Mutex::new(None),
        settled: Notify::new(),
        closing: Notify::new(),
        close_gate: Mutex::new(None),
        preparing: Notify::new(),
        prepare_gate: Mutex::new(None),
        sender: Mutex::new(sender),
        executions: AtomicUsize::new(0),
        steers: AtomicUsize::new(0),
    });
    let agent = attached_agent(
        Arc::new(ProbeFactory {
            backend: backend.clone(),
            receiver: Mutex::new(Some(receiver)),
        }),
        manager,
    )
    .await
    .unwrap();
    (agent, backend)
}
async fn reattach_after_explicit_close(agent: &Agent) {
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Absent);
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Attached);
}
async fn recover_after_automatic_stop(agent: &Agent) {
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Absent);
    let authorization = agent
        .authorize_attachment(AttachmentRequest::AutomaticRecovery)
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Attached);
}

#[tokio::test]
async fn recovered_probe_does_not_route_retired_generation_output_to_the_replacement() {
    let (agent, backend, _) = probe(false).await;
    let retired_sender = backend.sender.lock().unwrap().clone();
    agent.close(actor()).await.unwrap();
    reattach_after_explicit_close(&agent).await;

    assert!(retired_sender
        .send(ExecutionEvent::new(
            ExecutionId::new("retired-generation").unwrap(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        ))
        .is_err());
    agent.close(actor()).await.unwrap();
}
fn input(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new("message").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 10,
    }
}

#[tokio::test]
async fn uncertain_cleanup_blocks_admission_until_explicit_close_succeeds() {
    let (agent, backend, _) = probe(false).await;
    backend.fail_close.store(true, Ordering::SeqCst);
    assert_eq!(
        agent.close(close_action()).await,
        Err(AgentError::CleanupUncertain)
    );
    assert!(matches!(
        agent.enqueue(input("blocked"), close_action()).await,
        Err(AgentError::Closed)
    ));
    assert_eq!(
        agent.invoke(input("blocked-direct"), close_action()).await,
        Err(AgentError::Closed)
    );
    assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
    backend.fail_close.store(false, Ordering::SeqCst);
    agent.close(close_action()).await.unwrap();
    reattach_after_explicit_close(&agent).await;
    assert_eq!(
        agent
            .enqueue(input("safe"), close_action())
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
}

#[tokio::test]
async fn contradictory_terminal_observation_cannot_become_a_successful_receipt() {
    let (agent, _, storage) = probe(true).await;
    let result = agent
        .enqueue(input("contradiction"), close_action())
        .await
        .unwrap()
        .wait()
        .await;
    assert!(
        matches!(&result, Err(AgentError::ExecutionObservation { error, execution_result: Some(outcome) })
        if matches!(error.as_ref(), AgentError::Protocol(_)) && **outcome == Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(storage.snapshot().invocations[0].result, Some(result));
}

#[tokio::test]
async fn steering_during_preparation_enters_the_boundary_queue() {
    let (agent, backend, _) = probe(false).await;
    let (release, wait) = oneshot::channel();
    *backend.prepare_gate.lock().unwrap() = Some(wait);
    let first = agent
        .enqueue(input("preparing"), close_action())
        .await
        .unwrap();
    backend.preparing.notified().await;
    let correction = agent
        .steer(input("correction"), close_action())
        .await
        .unwrap();
    release.send(()).unwrap();
    let SteeringDelivery::Queued(correction) = correction else {
        panic!("preparation is not native execution")
    };
    assert_eq!(first.wait().await, Ok(ExecutionOutcome::Completed));
    assert_eq!(correction.wait().await, Ok(ExecutionOutcome::Completed));
    assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn restored_running_record_recovers_its_already_saved_result() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    agent
        .enqueue(input("saved"), close_action())
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    // Recreate the durable intermediate state between saving settlement and its
    // terminal scheduling edge, as can be observed after a process interruption.
    storage
        .0
        .lock()
        .unwrap()
        .snapshot
        .as_mut()
        .unwrap()
        .invocations[0]
        .scheduling
        .pop();
    let before = provider.calls.executions.load(Ordering::SeqCst);
    let restored = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(
        restored
            .enqueue(input("saved"), close_action())
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), before);
}

struct PauseAfterHook {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}
impl InvocationHook for PauseAfterHook {
    fn after_invocation(
        &self,
        _: &InvocationContext<'_>,
        _: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steering_after_provider_settlement_does_not_inject_during_after_hooks() {
    let (agent, backend, _) = probe(false).await;
    let (entered, waiting) = oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    agent.add_invocation_hook(Arc::new(PauseAfterHook {
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(released),
    }));
    let first = agent
        .enqueue(input("settled"), close_action())
        .await
        .unwrap();
    waiting.await.unwrap();
    let delivery = tokio::time::timeout(
        Duration::from_secs(2),
        agent.steer(input("later"), close_action()),
    )
    .await;
    release.send(()).unwrap();
    let SteeringDelivery::Queued(later) = delivery.unwrap().unwrap() else {
        panic!("after hooks are outside provider execution")
    };
    first.wait().await.unwrap();
    later.wait().await.unwrap();
    assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stopped_queue_retains_each_input_identity_and_the_triggering_failure() {
    let (agent, backend, storage) = probe(true).await;
    let (release, wait) = oneshot::channel();
    *backend.prepare_gate.lock().unwrap() = Some(wait);
    let first = agent
        .enqueue(input("failed"), close_action())
        .await
        .unwrap();
    backend.preparing.notified().await;
    let second = agent
        .enqueue(input("pending-a"), close_action())
        .await
        .unwrap();
    let third = agent
        .enqueue(input("pending-b"), close_action())
        .await
        .unwrap();
    release.send(()).unwrap();
    let failure = first.wait().await;
    assert!(matches!(
        failure,
        Err(AgentError::ExecutionObservation { .. })
    ));
    assert_eq!(second.wait().await, Err(AgentError::Closed));
    assert_eq!(third.wait().await, Err(AgentError::Closed));
    let snapshot = storage.snapshot();
    assert_eq!(
        snapshot.invocations[0].request.execution_id,
        ExecutionId::new("failed").unwrap()
    );
    assert_eq!(snapshot.invocations[0].result, Some(failure));
    for (record, id) in snapshot.invocations[1..]
        .iter()
        .zip(["pending-a", "pending-b"])
    {
        assert_eq!(record.request.execution_id, ExecutionId::new(id).unwrap());
        let edge = record.scheduling.last().unwrap();
        assert_eq!(edge.stage, InvocationStage::Cancelled);
        assert_eq!(edge.cause, SchedulingCause::RunnerStopped);
        assert!(edge.actor.is_none());
        assert!(edge.target.is_none());
    }
    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn restored_scheduling_from_a_custom_store_is_validated_before_provider_open() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let history = vec![
        InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(actor()),
        },
        InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: Some(InvocationStage::Running),
            stage: InvocationStage::Settled,
            cause: SchedulingCause::ExecutionSettled,
            actor: None,
        },
    ];
    assert!(history.iter().all(|event| event.transition().is_ok()));
    storage.0.lock().unwrap().snapshot = Some(SessionSnapshot {
        queue_history: Vec::new(),
        id: SessionId::new("conversation").unwrap(),
        provider: provider.identity(),
        provider_context: ProviderContext::Recorded(
            ExecutionSessionId::new("saved-context").unwrap(),
        ),
        invocations: vec![InvocationRecord {
            target_event_offset: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            cancellation: None,
            submission: SubmissionMode::Queued,
            request: request("corrupt"),
            actor: actor(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: history,
            result: Some(Ok(ExecutionOutcome::Completed)),
        }],
    });
    assert!(matches!(
        attached_agent(provider.clone(), storage.manager().await).await,
        Err(error) if matches!(error, AgentError::Storage(StorageError::Corrupt(_)))
    ));
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    assert!(provider.calls.opens.lock().unwrap().is_empty());
    assert_eq!(storage.0.lock().unwrap().writes, 0);
    assert_eq!(storage.snapshot().invocations[0].scheduling.len(), 2);
}

#[tokio::test]
async fn execution_cleanup_uncertainty_blocks_all_admission_including_wrapped_errors() {
    let errors = uncertain_cleanup_errors();
    for error in errors {
        let (agent, backend, _) = probe(false).await;
        backend.fail_close.store(true, Ordering::SeqCst);
        backend.suppress_terminal.store(true, Ordering::SeqCst);
        *backend.execution_attachment.lock().unwrap() = ProviderSessionState::CleanupRequired;
        *backend.execution_error.lock().unwrap() = Some(error.clone());
        assert_eq!(
            agent.invoke(input("uncertain"), actor()).await,
            Err(AgentError::ExecutionObservation {
                error: Box::new(AgentError::CleanupUncertain),
                execution_result: Some(Box::new(Err(error))),
            })
        );
        assert_eq!(
            agent.invoke(input("next"), actor()).await,
            Err(AgentError::Closed)
        );
        assert!(matches!(
            agent.enqueue(input("queued"), actor()).await,
            Err(AgentError::Closed)
        ));
        assert!(matches!(
            agent.steer(input("steer"), actor()).await,
            Err(AgentError::Closed)
        ));
        assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
        backend.fail_close.store(false, Ordering::SeqCst);
        agent.close(actor()).await.unwrap();
        backend.suppress_terminal.store(false, Ordering::SeqCst);
        *backend.execution_error.lock().unwrap() = None;
        *backend.execution_attachment.lock().unwrap() = ProviderSessionState::Usable;
        backend.execution_rejected.store(false, Ordering::SeqCst);
        reattach_after_explicit_close(&agent).await;
        agent.invoke(input("recovered"), actor()).await.unwrap();
    }
}

#[tokio::test]
async fn duplicate_terminal_never_replaces_the_first_observation_or_reports_success() {
    for contradictory in [true, false] {
        let (agent, backend, storage) = probe(contradictory).await;
        backend.duplicate_terminal.store(true, Ordering::SeqCst);
        let result = agent.invoke(input("duplicates"), actor()).await;
        assert!(
            matches!(&result, Err(AgentError::ExecutionObservation { error, .. })
            if matches!(error.as_ref(), AgentError::Protocol(message) if message == "duplicate terminal observation"))
        );
        let snapshot = storage.snapshot();
        let terminal: Vec<_> = snapshot.invocations[0]
            .events
            .iter()
            .filter_map(|event| match event.update() {
                ExecutionUpdate::Finished(outcome) => Some(*outcome),
                _ => None,
            })
            .collect();
        assert_eq!(
            terminal,
            vec![if contradictory {
                ExecutionOutcome::Cancelled
            } else {
                ExecutionOutcome::Completed
            }]
        );
        assert_eq!(snapshot.invocations[0].result, Some(result));
    }
}

#[tokio::test]
async fn failure_cleanup_closes_admission_before_awaiting_provider_shutdown() {
    let (agent, backend, storage) = probe(false).await;
    // A previous successful close must not be mistaken for a concurrent close.
    agent.close(actor()).await.unwrap();
    backend.closing.notified().await;
    reattach_after_explicit_close(&agent).await;
    let (release, gate) = oneshot::channel();
    *backend.close_gate.lock().unwrap() = Some(gate);
    let (settle, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    storage.0.lock().unwrap().fail_observation = Some("failing-save".into());
    let task = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("failing-save"), actor()).await }
    });
    backend.closing.notified().await;
    let steering = agent.steer(input("during-cleanup"), actor()).await;
    let queued = agent.enqueue(input("queued-cleanup"), actor()).await;
    release.send(()).unwrap();
    settle.send(()).unwrap();
    assert!(matches!(steering, Err(AgentError::Closed)));
    assert!(matches!(queued, Err(AgentError::Closed)));
    assert!(matches!(
        task.await.unwrap(),
        Err(AgentError::StorageAfterExecution { .. })
    ));
    assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
    assert_eq!(storage.snapshot().invocations.len(), 1);
    recover_after_automatic_stop(&agent).await;
    agent.invoke(input("after-cleanup"), actor()).await.unwrap();
}

#[tokio::test]
async fn custom_storage_rejects_terminal_corruption_before_open_and_preserves_known_failures() {
    for duplicate in [false, true] {
        let (agent, _, storage) = probe(false).await;
        agent.invoke(input("saved"), actor()).await.unwrap();
        drop(agent);
        {
            let mut state = storage.0.lock().unwrap();
            let record = &mut state.snapshot.as_mut().unwrap().invocations[0];
            if duplicate {
                record.events.push(record.events[0].clone());
            } else {
                record.result = Some(Ok(ExecutionOutcome::Cancelled));
            }
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        let (_, backend, _) = probe(false).await;
        drop(sender);
        assert!(matches!(
            attached_agent(
                Arc::new(ProbeFactory {
                    backend,
                    receiver: Mutex::new(Some(receiver))
                }),
                storage.manager().await
            )
            .await,
            Err(error) if matches!(error, AgentError::Storage(StorageError::Corrupt(_)))
        ));
    }
    let (agent, _, storage) = probe(true).await;
    assert!(agent.invoke(input("known-failure"), actor()).await.is_err());
    drop(agent);
    let (_, backend, _) = probe(false).await;
    let (_, receiver) = mpsc::unbounded_channel();
    attached_agent(
        Arc::new(ProbeFactory {
            backend,
            receiver: Mutex::new(Some(receiver)),
        }),
        storage.manager().await,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn custom_storage_cannot_restore_a_cancellation_without_its_original_request() {
    let (agent, _, storage) = probe(false).await;
    agent.invoke(input("saved-review"), actor()).await.unwrap();
    drop(agent);
    {
        let mut state = storage.0.lock().unwrap();
        let saved = state.snapshot.as_mut().unwrap();
        let decision = PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request());
        let options = PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow",
                decision.clone(),
            )
            .unwrap()],
            &PermissionOfferPolicy::new(vec![decision]).unwrap(),
        )
        .unwrap();
        let request = PermissionRequest::new(
            PermissionId::new("unknown-review").unwrap(),
            saved.invocations[0].request.execution_id.clone(),
            ToolCallId::new("tool").unwrap(),
            options,
        );
        let execution = request.execution_id().clone();
        let mut aggregate =
            ExecutionSession::new(saved.provider_context.recorded().unwrap().clone());
        aggregate.begin_execution(execution.clone()).unwrap();
        aggregate
            .observe_tool(
                &execution,
                ToolCallUpdate::new(request.tool_id().clone(), None, None, None, None, None),
            )
            .unwrap();
        aggregate.request_permission(request).unwrap();
        let request = aggregate
            .finish_execution(&execution, Ok(ExecutionOutcome::Completed))
            .unwrap()
            .1
            .remove(0);
        let cancellation = PermissionCancellation::from_record(
            saved.provider_context.recorded().unwrap().clone(),
            request,
            ToolReviewInput {
                name: "Read".into(),
                arguments_json: "{}".into(),
            },
            CancellationOrigin::Runtime,
        )
        .unwrap();
        let execution = saved.invocations[0].request.execution_id.clone();
        saved.invocations[0].events.insert(
            0,
            ExecutionEvent::new(
                execution,
                ExecutionUpdate::PermissionCancelled(cancellation),
            ),
        );
    }
    let (_, backend, _) = probe(false).await;
    let (_, receiver) = mpsc::unbounded_channel();
    assert!(matches!(
        attached_agent(
            Arc::new(ProbeFactory {
                backend,
                receiver: Mutex::new(Some(receiver))
            }),
            storage.manager().await
        )
        .await,
        Err(error) if matches!(error, AgentError::Storage(StorageError::Corrupt(_)))
    ));
}

#[tokio::test]
async fn terminal_observation_rejects_later_provider_output() {
    let updates = vec![
        ExecutionUpdate::Message(MessageChunk::text("late")),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("late-tool").unwrap(),
            None,
            None,
            None,
            None,
            None,
        )),
    ];
    for update in updates {
        let (agent, backend, storage) = probe(false).await;
        *backend.late_output.lock().unwrap() = Some(update);
        let result = agent.invoke(input("late-output"), actor()).await;
        assert!(
            matches!(&result, Err(AgentError::ExecutionObservation { error, .. }) if matches!(error.as_ref(), AgentError::Protocol(_)))
        );
        assert_eq!(storage.snapshot().invocations[0].events.len(), 1);
        assert_eq!(storage.snapshot().invocations[0].result, Some(result));
    }
}

#[tokio::test]
async fn confirmed_cleanup_removes_steering_target_before_delayed_execution_settlement() {
    let (agent, backend, storage) = probe(false).await;
    let (settle, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    storage.0.lock().unwrap().fail_observation = Some("delayed-settlement".into());
    let invocation = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("delayed-settlement"), actor()).await }
    });
    // On this single-thread runtime, the close future returns synchronously after
    // notifying. The invocation then clears its target and awaits the held result
    // before this task can resume. Cleanup and settlement are distinct boundaries.
    backend.closing.notified().await;
    assert!(!invocation.is_finished());
    let steering = agent
        .steer(input("after-confirmed-cleanup"), actor())
        .await
        .unwrap();
    assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
    let SteeringDelivery::Queued(receipt) = steering else {
        panic!("closed context must not remain a native steering target")
    };
    settle.send(()).unwrap();
    assert!(matches!(
        invocation.await.unwrap(),
        Err(AgentError::StorageAfterExecution { .. })
    ));
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn wrapped_errors_with_confirmed_cleanup_do_not_latch_admission_closed() {
    let errors = confirmed_cleanup_errors();
    for error in errors {
        let (agent, backend, _) = probe(false).await;
        *backend.execution_attachment.lock().unwrap() =
            ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                forced: false,
            }));
        *backend.execution_error.lock().unwrap() = Some(error.clone());
        assert_eq!(
            agent.invoke(input("known-error"), actor()).await,
            Err(expected_terminal_error(error))
        );
        *backend.execution_error.lock().unwrap() = None;
        *backend.execution_attachment.lock().unwrap() = ProviderSessionState::Usable;
        backend.execution_rejected.store(false, Ordering::SeqCst);
        recover_after_automatic_stop(&agent).await;
        assert_eq!(
            agent.invoke(input("next-safe-input"), actor()).await,
            Ok(ExecutionOutcome::Completed)
        );
    }
}

#[tokio::test]
async fn steering_admission_overtaken_by_close_never_calls_the_closed_provider() {
    let (agent, backend, storage) = probe(false).await;
    let (settle, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let invocation = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("active-before-close"), actor()).await }
    });
    backend.preparing.notified().await;
    let (saving, saved) = oneshot::channel();
    let (release, released) = oneshot::channel();
    storage.0.lock().unwrap().pause_save = Some((saving, released));
    let steering = tokio::spawn({
        let agent = agent.clone();
        async move { agent.steer(input("admitted-during-close"), actor()).await }
    });
    saved.await.unwrap();
    let close = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(actor()).await }
    });
    backend.closing.notified().await;
    release.send(()).unwrap();
    let delivery = steering.await.unwrap();
    settle.send(()).unwrap();
    invocation.await.unwrap().unwrap();
    close.await.unwrap().unwrap();
    assert!(matches!(delivery, Err(AgentError::Closed)));
    assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn steering_target_settled_during_admission_falls_back_to_boundary_queue() {
    let (agent, backend, storage) = probe(false).await;
    let (settle, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let invocation = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("settles-during-save"), actor()).await }
    });
    backend.preparing.notified().await;
    let (saving, saved) = oneshot::channel();
    let (release, released) = oneshot::channel();
    storage.0.lock().unwrap().pause_save = Some((saving, released));
    let steering = tokio::spawn({
        let agent = agent.clone();
        async move { agent.steer(input("saved-after-settlement"), actor()).await }
    });
    saved.await.unwrap();
    settle.send(()).unwrap();
    // Returning from execute releases the target and then waits behind admission
    // persistence before this current-thread task can resume.
    backend.settled.notified().await;
    release.send(()).unwrap();
    let delivery = steering.await.unwrap().unwrap();
    invocation.await.unwrap().unwrap();
    let SteeringDelivery::Queued(receipt) = delivery else {
        panic!("settled target must fall back to next invocation")
    };
    assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
}

#[tokio::test]
async fn completed_close_actor_is_not_reused_for_later_runtime_failure_cancellations() {
    let (agent, backend, storage) = probe(false).await;
    agent
        .close(ActionContext::new("historical-user", "old-surface", "old-close").unwrap())
        .await
        .unwrap();
    reattach_after_explicit_close(&agent).await;
    let (settle, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    *backend.execution_error.lock().unwrap() = Some(AgentError::CleanupUncertain);
    // The provider health report, not this diagnostic, stops admitted waiting work.
    *backend.execution_attachment.lock().unwrap() = ProviderSessionState::CleanupRequired;
    let invocation = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("runtime-failure"), actor()).await }
    });
    backend.preparing.notified().await;
    let queued = agent.enqueue(input("waiting"), actor()).await.unwrap();
    let (saving, saved) = oneshot::channel();
    let (release, released) = oneshot::channel();
    storage.0.lock().unwrap().pause_save = Some((saving, released));
    let steering = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .steer(input("overtaken-by-runtime-failure"), actor())
                .await
        }
    });
    saved.await.unwrap();
    settle.send(()).unwrap();
    backend.settled.notified().await;
    release.send(()).unwrap();
    assert!(matches!(steering.await.unwrap(), Err(AgentError::Closed)));
    assert_eq!(
        invocation.await.unwrap(),
        Err(expected_terminal_error(AgentError::CleanupUncertain))
    );
    assert_eq!(queued.wait().await, Err(AgentError::Closed));
    let snapshot = storage.snapshot();
    let queued = snapshot
        .invocations
        .iter()
        .find(|record| record.request.execution_id.as_str() == "waiting")
        .unwrap();
    let cancellation = queued.scheduling.last().unwrap();
    assert_eq!(cancellation.cause, SchedulingCause::RunnerStopped);
    assert!(cancellation.actor.is_none());
    let steering = snapshot
        .invocations
        .iter()
        .find(|record| record.request.execution_id.as_str() == "overtaken-by-runtime-failure")
        .unwrap();
    let failure = steering.scheduling.last().unwrap();
    assert_eq!(failure.cause, SchedulingCause::RunnerStopped);
    assert!(failure.actor.is_none());
    assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
}

fn uncertain_cleanup_errors() -> Vec<AgentError> {
    vec![
        AgentError::CleanupUncertain,
        AgentError::AuditAndCleanupFailure,
        AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Backpressure),
            cleanup_error: Box::new(AgentError::Deadline),
        },
        AgentError::PermissionAnswerDeliveryAndAuditFailure {
            delivery_error: Box::new(AgentError::Transport("write".into())),
            cleanup_error: Some(Box::new(AgentError::CleanupUncertain)),
        },
        AgentError::StorageDuringClose {
            error: StorageError::Io("write".into()),
            cleanup_result: Box::new(Err(AgentError::CleanupUncertain)),
        },
        AgentError::StorageInitialization {
            error: StorageError::Io("write".into()),
            cleanup_result: Box::new(Err(AgentError::CleanupUncertain)),
        },
        AgentError::AfterInvocationHooks {
            failures: vec![],
            execution_result: Box::new(Err(AgentError::CleanupUncertain)),
        },
        AgentError::ExecutionObservation {
            error: Box::new(AgentError::CleanupUncertain),
            execution_result: None,
        },
        AgentError::StorageAfterExecution {
            error: StorageError::Io("write".into()),
            execution_result: Box::new(Err(AgentError::CleanupUncertain)),
        },
        AgentError::ExecutionObservation {
            error: Box::new(AgentError::Protocol("reader".into())),
            execution_result: Some(Box::new(Err(AgentError::CleanupUncertain))),
        },
    ]
}

fn confirmed_cleanup_errors() -> Vec<AgentError> {
    vec![
        AgentError::AuditFailure,
        AgentError::PermissionAnswerDeliveryAndAuditFailure {
            delivery_error: Box::new(AgentError::Deadline),
            cleanup_error: None,
        },
        AgentError::StorageDuringClose {
            error: StorageError::Io("write".into()),
            cleanup_result: Box::new(Ok(CloseOutcome { forced: false })),
        },
        AgentError::StorageInitialization {
            error: StorageError::Io("write".into()),
            cleanup_result: Box::new(Ok(CloseOutcome { forced: false })),
        },
        AgentError::AfterInvocationHooks {
            failures: vec![],
            execution_result: Box::new(Ok(ExecutionOutcome::Completed)),
        },
        AgentError::ExecutionObservation {
            error: Box::new(AgentError::Protocol("observation".into())),
            execution_result: Some(Box::new(Ok(ExecutionOutcome::Completed))),
        },
    ]
}

#[derive(Clone, Copy, Debug)]
enum ProviderControl {
    Steer,
    Answer,
    CancelPermission,
}

async fn invoke_control(agent: &Agent, operation: ProviderControl) -> Result<(), AgentError> {
    match operation {
        ProviderControl::Steer => agent.steer(input("control"), actor()).await.map(|_| ()),
        ProviderControl::Answer => agent
            .answer_permission(PermissionAnswer {
                attribution: attribution(),
                execution_id: ExecutionId::new("active").unwrap(),
                id: PermissionId::new("review").unwrap(),
                option_id: PermissionOptionId::new("allow").unwrap(),
            })
            .await
            .map(|_| ())
            .map_err(|failure| failure.into_parts().0),
        ProviderControl::CancelPermission => agent
            .cancel_permission(PermissionCancellationRequest {
                execution_id: ExecutionId::new("active").unwrap(),
                id: PermissionId::new("review").unwrap(),
                reason: PermissionCancellationReason::custom(
                    CustomPermissionCancellationReason::new("guard", "Guard withdrew this review")
                        .unwrap(),
                ),
                actor: actor(),
            })
            .await
            .map(|_| ()),
    }
}

#[tokio::test]
async fn provider_controls_latch_every_uncertain_cleanup_wrapper_until_explicit_recovery() {
    for operation in [
        ProviderControl::Steer,
        ProviderControl::Answer,
        ProviderControl::CancelPermission,
    ] {
        for error in uncertain_cleanup_errors() {
            let (agent, backend, _) = probe(false).await;
            *backend.control_attachment.lock().unwrap() = ProviderSessionState::CleanupRequired;
            let (release, wait) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(wait);
            let active = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("active"), actor()).await }
            });
            backend.executing.notified().await;
            *backend.control_error.lock().unwrap() = Some(error.clone());
            // Native steering owns immediate cleanup. Keep it genuinely unconfirmed
            // so this test exercises the recovery fence rather than error spelling.
            backend.fail_close.store(true, Ordering::SeqCst);
            let expected = if matches!(operation, ProviderControl::Steer) {
                AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(error),
                    cleanup_error: Box::new(AgentError::CleanupUncertain),
                }
            } else {
                error
            };
            assert_eq!(
                invoke_control(&agent, operation).await,
                Err(expected),
                "{operation:?}"
            );
            release.send(()).unwrap();
            assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
            assert_eq!(
                agent.invoke(input("blocked-direct"), actor()).await,
                Err(AgentError::Closed)
            );
            assert!(matches!(
                agent.enqueue(input("blocked-queue"), actor()).await,
                Err(AgentError::Closed)
            ));
            assert!(matches!(
                agent.steer(input("blocked-steer"), actor()).await,
                Err(AgentError::Closed)
            ));
            assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
            assert_eq!(
                backend.steers.load(Ordering::SeqCst),
                usize::from(matches!(operation, ProviderControl::Steer))
            );
            backend.fail_close.store(true, Ordering::SeqCst);
            assert_eq!(
                agent.close(actor()).await,
                Err(AgentError::CleanupUncertain)
            );
            assert_eq!(
                agent.invoke(input("still-blocked"), actor()).await,
                Err(AgentError::Closed)
            );
            backend.fail_close.store(false, Ordering::SeqCst);
            agent.close(actor()).await.unwrap();
            *backend.control_error.lock().unwrap() = None;
            *backend.control_attachment.lock().unwrap() = ProviderSessionState::Usable;
            reattach_after_explicit_close(&agent).await;
            assert_eq!(
                agent.invoke(input("recovered"), actor()).await,
                Ok(ExecutionOutcome::Completed)
            );
        }
    }
}

#[tokio::test]
async fn provider_controls_with_confirmed_cleanup_do_not_latch_admission() {
    for operation in [
        ProviderControl::Steer,
        ProviderControl::Answer,
        ProviderControl::CancelPermission,
    ] {
        for error in confirmed_cleanup_errors() {
            let (agent, backend, _) = probe(false).await;
            *backend.control_attachment.lock().unwrap() =
                ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                    forced: false,
                }));
            let (release, wait) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(wait);
            let active = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("active"), actor()).await }
            });
            backend.executing.notified().await;
            *backend.control_error.lock().unwrap() = Some(error.clone());
            assert_eq!(
                invoke_control(&agent, operation).await,
                Err(error),
                "{operation:?}"
            );
            release.send(()).unwrap();
            assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
            *backend.control_error.lock().unwrap() = None;
            *backend.control_attachment.lock().unwrap() = ProviderSessionState::Usable;
            recover_after_automatic_stop(&agent).await;
            assert_eq!(
                agent.invoke(input("available"), actor()).await,
                Ok(ExecutionOutcome::Completed)
            );
            assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
        }
    }
}

#[tokio::test]
async fn permission_control_waiter_loss_does_not_lose_cleanup_uncertainty() {
    for operation in [ProviderControl::Answer, ProviderControl::CancelPermission] {
        for uncertain in [false, true] {
            let (agent, backend, _) = probe(false).await;
            *backend.control_attachment.lock().unwrap() = if uncertain {
                ProviderSessionState::CleanupRequired
            } else {
                ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                    forced: false,
                }))
            };
            *backend.control_error.lock().unwrap() = Some(if uncertain {
                AgentError::PermissionAnswerDeliveryAndAuditFailure {
                    delivery_error: Box::new(AgentError::Deadline),
                    cleanup_error: Some(Box::new(AgentError::CleanupUncertain)),
                }
            } else {
                AgentError::PermissionAnswerDeliveryAndAuditFailure {
                    delivery_error: Box::new(AgentError::Deadline),
                    cleanup_error: None,
                }
            });
            let (release, wait) = oneshot::channel();
            *backend.control_gate.lock().unwrap() = Some(wait);
            let caller = tokio::spawn({
                let agent = agent.clone();
                async move { invoke_control(&agent, operation).await }
            });
            backend.control_entered.notified().await;
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
            release.send(()).unwrap();
            // The detached control has observed its result and latched its barrier
            // without another suspension before this single-thread task resumes.
            backend.control_finished.notified().await;
            if uncertain {
                assert_eq!(
                    agent.invoke(input("blocked"), actor()).await,
                    Err(AgentError::Closed)
                );
                assert!(matches!(
                    agent.enqueue(input("blocked-queue"), actor()).await,
                    Err(AgentError::Closed)
                ));
                assert!(matches!(
                    agent.steer(input("blocked-steer"), actor()).await,
                    Err(AgentError::Closed)
                ));
                assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
                agent.close(actor()).await.unwrap();
                reattach_after_explicit_close(&agent).await;
            } else {
                recover_after_automatic_stop(&agent).await;
            }
            assert_eq!(
                agent.invoke(input("available"), actor()).await,
                Ok(ExecutionOutcome::Completed)
            );
        }
    }
}

async fn submit_with_identity(
    agent: &Agent,
    mode: usize,
    raw: String,
) -> Result<ExecutionOutcome, ExecutionError> {
    let request = ExecutionRequest {
        execution_id: ExecutionId::new(raw)?,
        ..input("template")
    };
    Ok(match mode {
        0 => agent.invoke(request, close_action()).await.unwrap(),
        1 => agent
            .enqueue(request, close_action())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap(),
        2 => agent
            .enqueue_steering(request, close_action())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap(),
        _ => match agent.steer(request, close_action()).await.unwrap() {
            SteeringDelivery::Queued(receipt) => receipt.wait().await.unwrap(),
            SteeringDelivery::Injected { .. } => panic!("idle steering must queue"),
        },
    })
}

#[tokio::test]
async fn execution_identity_bound_precedes_every_admission_path_and_exact_limit_runs() {
    let (agent, backend, storage) = probe(false).await;
    for mode in 0..4 {
        let writes = storage.0.lock().unwrap().writes;
        let dispatches = backend.executions.load(Ordering::SeqCst);
        let records = storage.snapshot().invocations.len();
        assert!(matches!(
            submit_with_identity(&agent, mode, "x".repeat(ExecutionId::MAX_BYTES + 1)).await,
            Err(ExecutionError::ValueTooLong { .. })
        ));
        assert_eq!(storage.0.lock().unwrap().writes, writes);
        assert_eq!(backend.executions.load(Ordering::SeqCst), dispatches);
        assert_eq!(storage.snapshot().invocations.len(), records);
        assert_eq!(
            submit_with_identity(
                &agent,
                mode,
                format!("{mode}{}", "x".repeat(ExecutionId::MAX_BYTES - 1))
            )
            .await,
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(backend.executions.load(Ordering::SeqCst), dispatches + 1);
        assert_eq!(storage.snapshot().invocations.len(), records + 1);
    }
}

#[tokio::test]
async fn queued_observations_are_rejected_before_persistence_or_publication() {
    for update in [
        ExecutionUpdate::Message(MessageChunk::text("fabricated queued output")),
        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
    ] {
        let (agent, backend, storage) = probe(false).await;
        let mut updates = agent.subscribe();
        let (release, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let first = {
            let agent = agent.clone();
            tokio::spawn(async move { agent.invoke(input("active"), actor()).await })
        };
        backend.executing.notified().await;
        // Wait until the legitimate event was persisted and published.
        assert_eq!(
            updates
                .next()
                .await
                .unwrap()
                .unwrap()
                .execution_id()
                .as_str(),
            "active"
        );
        let queued = agent.enqueue(input("undispatched"), actor()).await.unwrap();
        backend
            .sender
            .lock()
            .unwrap()
            .send(ExecutionEvent::new(queued.id().clone(), update))
            .unwrap();
        backend.closing.notified().await;
        assert!(storage.snapshot().invocations[1].events.is_empty());
        release.send(()).unwrap();
        assert!(first.await.unwrap().is_err());
        // Confirmed cleanup permits the legitimate queued invocation to run.
        queued.wait().await.unwrap();
        assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
        let saved = storage.snapshot();
        assert_eq!(saved.invocations[1].events.len(), 1);
        assert!(matches!(
            saved.invocations[1].events[0].update(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed)
        ));
        assert_eq!(
            updates
                .next()
                .await
                .unwrap()
                .unwrap()
                .execution_id()
                .as_str(),
            "undispatched"
        );
        // Cleanup and the legitimate follow-up have both settled.
        tokio::select! {
            biased;
            event = updates.next() => panic!("unexpected publication: {event:?}"),
            _ = async {} => {}
        }
    }
}

#[tokio::test]
async fn late_observations_keep_their_dispatched_owner_after_caller_loss() {
    let (agent, backend, storage) = probe(false).await;
    backend.suppress_terminal.store(true, Ordering::SeqCst);
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let abandoned = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.invoke(input("abandoned"), actor()).await })
    };
    backend.executing.notified().await;
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());
    backend
        .sender
        .lock()
        .unwrap()
        .send(ExecutionEvent::new(
            ExecutionId::new("abandoned").unwrap(),
            ExecutionUpdate::Message(MessageChunk::text("late evidence")),
        ))
        .unwrap();
    backend
        .sender
        .lock()
        .unwrap()
        .send(ExecutionEvent::new(
            ExecutionId::new("abandoned").unwrap(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        ))
        .unwrap();
    backend.suppress_terminal.store(false, Ordering::SeqCst);
    assert_eq!(
        agent.invoke(input("overlap"), actor()).await,
        Err(AgentError::Busy)
    );
    let next = agent.enqueue(input("next"), actor()).await.unwrap();
    release.send(()).unwrap();
    next.wait().await.unwrap();
    let saved = storage.snapshot();
    assert_eq!(
        saved.invocations[0].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(saved.invocations[0].events.len(), 2);
    assert_eq!(
        saved.invocations[0].events[0].execution_id().as_str(),
        "abandoned"
    );
    assert_eq!(saved.invocations[1].events.len(), 1);
}

#[tokio::test]
async fn admission_rejection_does_not_authorize_late_provider_output() {
    let (agent, backend, storage) = probe(false).await;
    backend.execution_rejected.store(true, Ordering::SeqCst);
    backend.suppress_terminal.store(true, Ordering::SeqCst);
    *backend.execution_error.lock().unwrap() =
        Some(AgentError::InvalidInput("not admitted".into()));
    assert!(matches!(
        agent.invoke(input("rejected"), actor()).await,
        Err(AgentError::InvalidInput(_))
    ));
    backend
        .sender
        .lock()
        .unwrap()
        .send(ExecutionEvent::new(
            ExecutionId::new("rejected").unwrap(),
            ExecutionUpdate::Message(MessageChunk::text("fabricated late output")),
        ))
        .unwrap();
    *backend.execution_error.lock().unwrap() = None;
    *backend.execution_attachment.lock().unwrap() = ProviderSessionState::Usable;
    backend.execution_rejected.store(false, Ordering::SeqCst);
    backend.suppress_terminal.store(false, Ordering::SeqCst);
    assert!(agent.invoke(input("later"), actor()).await.is_err());
    assert!(storage.snapshot().invocations[0].events.is_empty());
}

#[tokio::test]
async fn custom_storage_cannot_restore_observations_before_queue_dispatch() {
    let (agent, _, storage) = probe(false).await;
    agent.invoke(input("saved"), actor()).await.unwrap();
    agent.close(actor()).await.unwrap();
    drop(agent);
    {
        let mut state = storage.0.lock().unwrap();
        let record = &mut state.snapshot.as_mut().unwrap().invocations[0];
        record.submission = SubmissionMode::Queued;
        record.result = None;
        record.scheduling = vec![InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(record.actor.clone()),
        }];
    }
    let provider = TestProvider::new();
    let writes = storage.0.lock().unwrap().writes;
    assert!(
        matches!(attached_agent(provider.clone(), storage.manager().await).await,
        Err(error) if matches!(error, AgentError::Storage(StorageError::Corrupt(_))))
    );
    assert!(provider.calls.opens.lock().unwrap().is_empty());
    assert_eq!(storage.0.lock().unwrap().writes, writes);
}

#[tokio::test]
async fn dispatch_and_buffered_observation_cannot_deadlock_behind_an_admission_save() {
    let (agent, backend, storage) = probe(false).await;
    let (prepare_release, prepare_wait) = oneshot::channel();
    *backend.prepare_gate.lock().unwrap() = Some(prepare_wait);
    let mut first = agent.invoke(input("preparing-dispatch"), actor());
    tokio::select! {
        biased;
        result = &mut first => panic!("preparation unexpectedly settled: {result:?}"),
        _ = backend.preparing.notified() => {}
    }
    let (saving, save_release) = storage.pause_next_save();
    let queued = tokio::spawn({
        let agent = agent.clone();
        async move { agent.enqueue(input("queued-behind-save"), actor()).await }
    });
    saving.await.unwrap();
    *backend.early_output.lock().unwrap() = Some(ExecutionUpdate::Message(MessageChunk::text(
        "buffered observation",
    )));
    prepare_release.send(()).unwrap();
    // Poll the invocation while Evidence is exclusively held by admission I/O.
    // The old asynchronous dispatch marker queued ahead of event persistence;
    // selecting that ready event then stopped polling the marker forever.
    poll_fn(|cx| {
        assert!(first.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    save_release.send(()).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(1), first).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    let queued = queued.await.unwrap().unwrap();
    assert_eq!(
        timeout(Duration::from_secs(1), queued.wait())
            .await
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    timeout(Duration::from_secs(1), agent.close(actor()))
        .await
        .unwrap()
        .unwrap();
    let snapshot = storage.snapshot();
    assert_eq!(snapshot.invocations[0].events.len(), 2);
    assert_eq!(snapshot.invocations[1].events.len(), 1);
    assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn caller_loss_after_rejection_cannot_leave_observation_authority_behind_a_save() {
    let (agent, backend, storage) = probe(false).await;
    backend.execution_rejected.store(true, Ordering::SeqCst);
    backend.suppress_terminal.store(true, Ordering::SeqCst);
    *backend.execution_error.lock().unwrap() =
        Some(AgentError::InvalidInput("not admitted".into()));
    let (execution_release, execution_wait) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(execution_wait);
    let mut first = agent.invoke(input("rejected-before-caller-loss"), actor());
    tokio::select! {
        biased;
        result = &mut first => panic!("execution unexpectedly settled: {result:?}"),
        _ = backend.executing.notified() => {}
    }
    let (saving, save_release) = storage.pause_next_save();
    let queued = tokio::spawn({
        let agent = agent.clone();
        async move { agent.enqueue(input("after-rejection"), actor()).await }
    });
    saving.await.unwrap();
    execution_release.send(()).unwrap();
    backend.settled.notified().await;
    // Provider rejection is observed now, before the caller is lost. Revoking
    // dispatch authority must not wait for this unrelated admission write.
    poll_fn(|cx| {
        assert!(first.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(first);
    backend
        .sender
        .lock()
        .unwrap()
        .send(ExecutionEvent::new(
            ExecutionId::new("rejected-before-caller-loss").unwrap(),
            ExecutionUpdate::Message(MessageChunk::text("fabricated rejected output")),
        ))
        .unwrap();
    *backend.execution_error.lock().unwrap() = None;
    *backend.execution_attachment.lock().unwrap() = ProviderSessionState::Usable;
    backend.execution_rejected.store(false, Ordering::SeqCst);
    backend.suppress_terminal.store(false, Ordering::SeqCst);
    save_release.send(()).unwrap();
    let queued = queued.await.unwrap().unwrap();
    assert!(timeout(Duration::from_secs(1), queued.wait())
        .await
        .unwrap()
        .is_err());
    assert!(storage.snapshot().invocations[0].events.is_empty());
    timeout(Duration::from_secs(1), agent.close(actor()))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn shutdown_rejects_late_permission_controls_and_releases_admitted_response_waits() {
    for operation in [ProviderControl::Answer, ProviderControl::CancelPermission] {
        for abandon_waiter in [false, true] {
            let (agent, backend, _) = probe(false).await;
            let (control_release, control_wait) = oneshot::channel();
            *backend.control_gate.lock().unwrap() = Some(control_wait);
            let caller = tokio::spawn({
                let agent = agent.clone();
                async move { invoke_control(&agent, operation).await }
            });
            backend.control_entered.notified().await;
            let caller = if abandon_waiter {
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
                None
            } else {
                Some(caller)
            };
            let (close_release, close_wait) = oneshot::channel();
            *backend.close_gate.lock().unwrap() = Some(close_wait);
            backend.fail_close.store(true, Ordering::SeqCst);
            let closing = tokio::spawn({
                let agent = agent.clone();
                async move { agent.close(actor()).await }
            });
            backend.closing.notified().await;
            for late in [ProviderControl::Answer, ProviderControl::CancelPermission] {
                assert_eq!(invoke_control(&agent, late).await, Err(AgentError::Closed));
            }
            assert_eq!(backend.controls.load(Ordering::SeqCst), 1);
            if let Some(caller) = caller {
                assert_eq!(
                    timeout(Duration::from_secs(1), caller)
                        .await
                        .unwrap()
                        .unwrap(),
                    Err(AgentError::Closed)
                );
            }
            // Stop need not release this response gate to perform provider cleanup.
            close_release.send(()).unwrap();
            assert_eq!(
                timeout(Duration::from_secs(1), closing)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(AgentError::CleanupUncertain)
            );
            assert_eq!(
                invoke_control(&agent, operation).await,
                Err(AgentError::Closed)
            );
            assert_eq!(backend.controls.load(Ordering::SeqCst), 1);
            backend.fail_close.store(false, Ordering::SeqCst);
            agent.close(close_action()).await.unwrap();
            assert_eq!(backend.closes.load(Ordering::SeqCst), 2);
            assert_eq!(
                *backend.close_requests.lock().unwrap(),
                vec![SessionCloseRequest::Explicit(actor()); 2]
            );
            drop(control_release);
        }
    }
}

#[tokio::test]
async fn failure_and_explicit_close_share_cleanup_while_native_steering_is_waiting() {
    for explicit_first in [false, true] {
        let (agent, backend, storage) = probe(false).await;
        let mut updates = agent.subscribe();
        let (execution_release, execution_wait) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(execution_wait);
        let invocation = tokio::spawn({
            let agent = agent.clone();
            async move { agent.invoke(input("active"), actor()).await }
        });
        backend.executing.notified().await;
        assert!(matches!(
            updates.next().await.unwrap().unwrap().update(),
            ExecutionUpdate::Finished(_)
        ));
        let (control_release, control_wait) = oneshot::channel();
        *backend.control_gate.lock().unwrap() = Some(control_wait);
        let steering = tokio::spawn({
            let agent = agent.clone();
            async move { agent.steer(input("in-flight-steering"), actor()).await }
        });
        backend.control_entered.notified().await;
        let (close_release, close_wait) = oneshot::channel();
        *backend.close_gate.lock().unwrap() = Some(close_wait);
        let mut explicit = None;
        if explicit_first {
            explicit = Some(tokio::spawn({
                let agent = agent.clone();
                async move { agent.close(actor()).await }
            }));
            backend.closing.notified().await;
        }
        backend
            .sender
            .lock()
            .unwrap()
            .send(ExecutionEvent::new(
                ExecutionId::new("active").unwrap(),
                ExecutionUpdate::Message(MessageChunk::text("invalid output after terminal")),
            ))
            .unwrap();
        if !explicit_first {
            backend.closing.notified().await;
            explicit = Some(tokio::spawn({
                let agent = agent.clone();
                async move { agent.close(actor()).await }
            }));
        }
        let delivery = timeout(Duration::from_secs(1), steering)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(delivery, Err(AgentError::Closed)),
            "uncertain delivery must not become a queued fallback"
        );
        assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
        assert_eq!(
            invoke_control(&agent, ProviderControl::Answer).await,
            Err(AgentError::Closed)
        );
        // Confirmed provider cleanup releases its execution settlement; the
        // Agent must be allowed to observe it before either close waiter joins.
        execution_release.send(()).unwrap();
        close_release.send(()).unwrap();
        let result = timeout(Duration::from_secs(1), invocation)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result,
            Err(AgentError::ExecutionObservation {
                error: Box::new(AgentError::Protocol(
                    "provider output follows terminal observation".into()
                )),
                execution_result: Some(Box::new(Ok(ExecutionOutcome::Completed))),
            })
        );
        assert_eq!(storage.snapshot().invocations[0].result, Some(result));
        timeout(Duration::from_secs(1), explicit.unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
        let expected = if explicit_first {
            SessionCloseRequest::Explicit(actor())
        } else {
            SessionCloseRequest::ExecutionFailed
        };
        assert_eq!(*backend.close_requests.lock().unwrap(), vec![expected]);
        assert_eq!(
            storage.snapshot().invocations[1].result,
            Some(Err(AgentError::Closed))
        );
        assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
        drop(control_release);
        reattach_after_explicit_close(&agent).await;
        agent.invoke(input("restored"), actor()).await.unwrap();
        agent.close(actor()).await.unwrap();
        assert_eq!(
            backend.closes.load(Ordering::SeqCst),
            2,
            "new attachment cannot reuse the previous cleanup result"
        );
    }
}

#[tokio::test]
async fn concurrent_close_waiters_share_cleanup_even_when_the_first_waiter_is_dropped() {
    let (agent, backend, _) = probe(false).await;
    let (release, wait) = oneshot::channel();
    *backend.close_gate.lock().unwrap() = Some(wait);
    let mut first = agent.close(actor());
    tokio::select! {
        biased;
        result = &mut first => panic!("close unexpectedly settled: {result:?}"),
        _ = backend.closing.notified() => {}
    }
    let mut second = agent.close(close_action());
    // First-poll admission transfers ownership to the close supervisor.
    poll_fn(|cx| {
        assert!(second.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(first);
    assert_eq!(
        invoke_control(&agent, ProviderControl::Answer).await,
        Err(AgentError::Closed)
    );
    release.send(()).unwrap();
    timeout(Duration::from_secs(1), second)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        *backend.close_requests.lock().unwrap(),
        vec![SessionCloseRequest::Explicit(actor())]
    );
    reattach_after_explicit_close(&agent).await;
    agent
        .invoke(input("new-close-generation"), actor())
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    assert_eq!(backend.closes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn message_chunks_are_bounded_before_snapshot_copy_and_publication() {
    for thought in [false, true] {
        for (size, capacity) in [
            (0, 0),
            (
                ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES,
                ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES,
            ),
            (
                ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES + 1,
                ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES + 1,
            ),
            (1, ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES + 1),
        ] {
            let (agent, backend, storage) = probe(false).await;
            backend.suppress_terminal.store(true, Ordering::SeqCst);
            let mut updates = agent.subscribe();
            let (release, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            let invocation = {
                let agent = agent.clone();
                tokio::spawn(async move { agent.invoke(input("chunk"), actor()).await })
            };
            backend.executing.notified().await;
            let mut text = String::with_capacity(capacity);
            text.push_str(&"é".repeat(size / 2));
            if size % 2 != 0 {
                text.push('x');
            }
            let chunk = if thought {
                MessageChunk::thought(text)
            } else {
                MessageChunk::text(text)
            };
            backend
                .sender
                .lock()
                .unwrap()
                .send(ExecutionEvent::new(
                    ExecutionId::new("chunk").unwrap(),
                    ExecutionUpdate::Message(chunk),
                ))
                .unwrap();
            if size > ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES {
                backend.closing.notified().await;
                // The invalid event never reaches saved evidence or the UI.
                assert!(storage.snapshot().invocations[0].events.is_empty());
                release.send(()).unwrap();
                let error = invocation.await.unwrap().unwrap_err();
                assert!(
                    matches!(&error, AgentError::ExecutionObservation { error, execution_result: Some(settled) } if matches!(error.as_ref(), AgentError::Protocol(_)) && settled.as_ref() == &Ok(ExecutionOutcome::Completed)),
                    "{error:?}"
                );
                assert_eq!(storage.snapshot().invocations[0].result, Some(Err(error)));
                assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
                tokio::select! {
                    biased;
                    event = updates.next() => panic!("invalid chunk published: {event:?}"),
                    _ = async {} => {}
                }
            } else {
                let event = updates.next().await.unwrap().unwrap();
                let ExecutionUpdate::Message(chunk) = event.update() else {
                    panic!("expected chunk");
                };
                assert_eq!(chunk.payload_bytes(), size);
                assert_eq!(
                    chunk.kind(),
                    if thought {
                        MessageKind::Thought
                    } else {
                        MessageKind::Text
                    }
                );
                backend
                    .sender
                    .lock()
                    .unwrap()
                    .send(ExecutionEvent::new(
                        ExecutionId::new("chunk").unwrap(),
                        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
                    ))
                    .unwrap();
                release.send(()).unwrap();
                assert_eq!(invocation.await.unwrap(), Ok(ExecutionOutcome::Completed));
                assert_eq!(storage.snapshot().invocations[0].events.len(), 2);
                agent.close(actor()).await.unwrap();
            }
        }
    }
}

fn expected_terminal_error(settlement: AgentError) -> AgentError {
    AgentError::ExecutionObservation {
        error: Box::new(AgentError::Protocol(
            "terminal observation contradicts execution settlement".into(),
        )),
        execution_result: Some(Box::new(Err(settlement))),
    }
}

#[tokio::test]
async fn completed_control_with_unpolled_waiter_does_not_hold_close_open() {
    for operation in [ProviderControl::Answer, ProviderControl::CancelPermission] {
        let (agent, backend, _) = probe(false).await;
        let mut waiter = Box::pin(invoke_control(&agent, operation));
        poll_fn(|cx| {
            assert!(waiter.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        backend.control_finished.notified().await;
        // Keep the response future alive but do not poll it again. Only actual
        // SDK work, not an idle consumer, belongs to the shutdown join.
        timeout(Duration::from_secs(2), agent.close(actor()))
            .await
            .expect("completed control waiter cannot own shutdown work")
            .unwrap();
        assert_eq!(waiter.await, Err(AgentError::StalePermission));
        assert_eq!(backend.controls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn explicit_recovery_ignores_historical_operation_error_when_cleanup_succeeds() {
    let (agent, backend, _) = probe(false).await;
    *backend.control_attachment.lock().unwrap() = ProviderSessionState::CleanupRequired;
    assert_eq!(
        invoke_control(&agent, ProviderControl::Answer).await,
        Err(AgentError::StalePermission)
    );
    assert_eq!(
        agent.invoke(input("blocked"), actor()).await,
        Err(AgentError::Closed)
    );
    *backend.cleanup_report.lock().unwrap() = Some(
        CleanupReport::confirmed(CloseOutcome { forced: false }).with_operation_failure(Some(
            AgentError::Protocol("historical operation failure".into()),
        )),
    );
    assert_eq!(
        agent.close(actor()).await,
        Ok(CloseOutcome { forced: false })
    );
    reattach_after_explicit_close(&agent).await;
    assert_eq!(
        agent.invoke(input("recovered"), actor()).await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn confirmed_control_cleanup_retires_native_target_before_execution_settlement() {
    let (agent, backend, _) = probe(false).await;
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let invocation = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("active"), actor()).await }
    });
    backend.executing.notified().await;
    *backend.control_attachment.lock().unwrap() =
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        }));
    assert_eq!(
        invoke_control(&agent, ProviderControl::Answer).await,
        Err(AgentError::StalePermission)
    );
    let delivery = agent.steer(input("following"), actor()).await.unwrap();
    let steers = backend.steers.load(Ordering::SeqCst);
    release.send(()).unwrap();
    assert_eq!(invocation.await.unwrap(), Ok(ExecutionOutcome::Completed));
    assert_eq!(
        steers, 0,
        "confirmed resource cleanup retires the native target"
    );
    let SteeringDelivery::Queued(receipt) = delivery else {
        panic!("cleaned target must fall back to a boundary invocation");
    };
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(actor()).await.unwrap();
}
