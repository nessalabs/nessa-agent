mod conformance;
mod initialization;
mod lifecycle;
mod messages;
mod observation_settlement;
mod retained_output;
mod review_regressions;
mod robustness;
mod streaming_persistence;
mod structured_observations;
use super::support::*;
use nessa_sdk::application::agent_execution::executions::SubmissionMode;
use nessa_sdk::application::agent_execution::sessions::QueueHistoryRecord;
use nessa_sdk::application::agent_execution::{
    agents::Agent,
    hooks::{HookError, InvocationContext, InvocationHook},
    providers::{ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest},
};
use nessa_sdk::domain::agent_execution::executions::QueueMutation;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Default)]
struct SavedState {
    leased: bool,
    pause_save: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    pause_queue: Option<(QueueMutation, oneshot::Sender<()>, oneshot::Receiver<()>)>,
    snapshot: Option<SessionSnapshot>,
    writes: usize,
    fail_write: Option<usize>,
    fail_queue: Option<QueueMutation>,
    panic_queue: Option<QueueMutation>,
    fail_scheduling: Option<(String, InvocationStage)>,
    fail_observation: Option<String>,
    fail_settlement: Option<String>,
    panic_provider_settlement: Option<bool>,
}
#[derive(Clone, Default)]
pub(super) struct MemoryStorage(Arc<Mutex<SavedState>>);
struct MemoryStore(Arc<Mutex<SavedState>>);
impl Drop for MemoryStore {
    fn drop(&mut self) {
        self.0.lock().unwrap().leased = false;
    }
}
impl SessionStorage for MemoryStorage {
    fn open(&self, _: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            let mut state = self.0.lock().unwrap();
            if state.leased {
                return Err(StorageError::Busy);
            }
            state.leased = true;
            Ok(Box::new(MemoryStore(self.0.clone())) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for MemoryStore {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        Box::pin(async { Ok(self.0.lock().unwrap().snapshot.clone()) })
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            let pause = self.0.lock().unwrap().pause_save.take();
            if let Some((started, release)) = pause {
                let _ = started.send(());
                let _ = release.await;
            }
            let queue_pause = {
                let mut state = self.0.lock().unwrap();
                if state.pause_queue.as_ref().is_some_and(|(mutation, _, _)| {
                    snapshot
                        .queue_history
                        .last()
                        .is_some_and(|entry| &entry.mutation == mutation)
                }) {
                    state.pause_queue.take()
                } else {
                    None
                }
            };
            if let Some((_, started, release)) = queue_pause {
                let _ = started.send(());
                let _ = release.await;
            }
            let mut state = self.0.lock().unwrap();
            state.writes += 1;
            if state.panic_queue.as_ref().is_some_and(|mutation| {
                snapshot
                    .queue_history
                    .last()
                    .is_some_and(|entry| &entry.mutation == mutation)
            }) {
                state.panic_queue = None;
                drop(state);
                panic!("queue membership persistence panic");
            }
            let fail_queue = state.fail_queue.as_ref().is_some_and(|mutation| {
                snapshot
                    .queue_history
                    .last()
                    .is_some_and(|entry| &entry.mutation == mutation)
            });
            if fail_queue {
                state.fail_queue = None;
            }

            let fail_scheduling = state.fail_scheduling.as_ref().is_some_and(|(id, stage)| {
                snapshot.invocations.iter().any(|record| {
                    record.request.execution_id.as_str() == id
                        && record
                            .scheduling
                            .last()
                            .is_some_and(|event| event.stage == *stage)
                })
            });
            if fail_scheduling {
                state.fail_scheduling = None;
            }
            let fail_observation = state.fail_observation.as_ref().is_some_and(|id| {
                snapshot.invocations.iter().any(|record| {
                    record.request.execution_id.as_str() == id && !record.events.is_empty()
                })
            });
            let fail_settlement = state.fail_settlement.as_ref().is_some_and(|id| {
                snapshot.invocations.iter().any(|record| {
                    record.request.execution_id.as_str() == id && record.result.is_some()
                })
            });
            if fail_observation {
                state.fail_observation = None;
            }
            if fail_settlement {
                state.fail_settlement = None;
            }
            if fail_queue
                || state.fail_write == Some(state.writes)
                || fail_scheduling
                || fail_observation
                || fail_settlement
            {
                return Err(StorageError::Io("fixture write failed".into()));
            }
            let provider_settlement_checkpoint = snapshot
                .invocations
                .iter()
                .any(|record| record.provider_report.is_some() && record.result.is_none());
            if provider_settlement_checkpoint {
                if let Some(commit_first) = state.panic_provider_settlement.take() {
                    if commit_first {
                        state.snapshot = Some(snapshot);
                    }
                    drop(state);
                    panic!("provider settlement save panic, committed={commit_first}");
                }
            }
            state.snapshot = Some(snapshot);
            Ok(())
        })
    }
}
impl MemoryStorage {
    pub(super) fn pause_next_save(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (started, observing) = oneshot::channel();
        let (release, waiting) = oneshot::channel();
        self.0.lock().unwrap().pause_save = Some((started, waiting));
        (observing, release)
    }

    pub(super) async fn manager(&self) -> SessionManager {
        SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(self.clone()),
        )
        .await
        .unwrap()
    }
    pub(super) fn snapshot(&self) -> SessionSnapshot {
        self.0.lock().unwrap().snapshot.clone().unwrap()
    }
    pub(super) fn fail_scheduling(&self, id: &str, stage: InvocationStage) {
        self.0.lock().unwrap().fail_scheduling = Some((id.into(), stage));
    }
    pub(super) fn fail_next(&self) {
        let mut state = self.0.lock().unwrap();
        state.fail_write = Some(state.writes + 1);
    }
}

#[derive(Default)]
struct ProviderCalls {
    opens: Mutex<Vec<Option<ExecutionSessionId>>>,
    executions: AtomicUsize,
    closes: Mutex<Vec<SessionCloseRequest>>,
    order: Mutex<Vec<String>>,
}
struct TestProvider {
    identity: ProviderIdentity,
    calls: Arc<ProviderCalls>,
    wait_for_close: bool,
    outcome: Result<ExecutionOutcome, AgentError>,
}
impl TestProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            identity: ProviderIdentity::new("fixture", "fixture", "workspace").unwrap(),
            calls: Arc::new(ProviderCalls::default()),
            wait_for_close: false,
            outcome: Ok(ExecutionOutcome::Completed),
        })
    }
}
struct TestBackend {
    calls: Arc<ProviderCalls>,
    sender: Mutex<Option<mpsc::UnboundedSender<ExecutionEvent>>>,
    closing: watch::Sender<bool>,
    wait_for_close: bool,
    outcome: Result<ExecutionOutcome, AgentError>,
}
struct TestEvents(mpsc::UnboundedReceiver<ExecutionEvent>);
impl ExecutionEventStream for TestEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async { Ok(self.0.recv().await) })
    }
}
impl AgentProvider for TestProvider {
    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        let (restore, _control) = request.into_parts();
        Box::pin(async move {
            self.calls.opens.lock().unwrap().push(restore.clone());
            let id =
                restore.unwrap_or_else(|| ExecutionSessionId::new("provider-context").unwrap());
            let (sender, receiver) = mpsc::unbounded_channel();
            let (closing, _) = watch::channel(false);
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    id,
                    Arc::new(TestBackend {
                        calls: self.calls.clone(),
                        sender: Mutex::new(Some(sender)),
                        closing,
                        wait_for_close: self.wait_for_close,
                        outcome: self.outcome.clone(),
                    }),
                    capabilities(),
                ),
                events: Box::new(TestEvents(receiver)),
            })
        })
    }
}
impl ProviderSessionBackend for TestBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> = {
                async move {
                    self.calls.executions.fetch_add(1, Ordering::SeqCst);
                    self.calls.order.lock().unwrap().push("execute".into());
                    let sender = self
                        .sender
                        .lock()
                        .unwrap()
                        .clone()
                        .ok_or(AgentError::Closed)?;
                    sender
                        .send(ExecutionEvent::new(
                            input.execution_id.clone(),
                            ExecutionUpdate::Message(MessageChunk::text(
                                input.user_message.text_str(),
                            )),
                        ))
                        .map_err(|_| AgentError::Backpressure)?;
                    let result = if self.wait_for_close {
                        let mut closing = self.closing.subscribe();
                        while !*closing.borrow() {
                            closing.changed().await.map_err(|_| AgentError::Closed)?;
                        }
                        Ok(ExecutionOutcome::Cancelled)
                    } else {
                        self.outcome.clone()
                    };
                    if let Ok(outcome) = result {
                        sender
                            .send(ExecutionEvent::new(
                                input.execution_id,
                                ExecutionUpdate::Finished(outcome),
                            ))
                            .map_err(|_| AgentError::Backpressure)?;
                    } else {
                        self.sender.lock().unwrap().take();
                    }
                    result
                }
            }
            .await;
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, origin: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            let result: Result<CloseOutcome, AgentError> = {
                async move {
                    self.calls.closes.lock().unwrap().push(origin);
                    self.closing.send_replace(true);
                    self.sender.lock().unwrap().take();
                    Ok(CloseOutcome { forced: false })
                }
            }
            .await;
            match result {
                Ok(outcome) => CleanupReport::confirmed(outcome),
                Err(error) => CleanupReport::unconfirmed(error),
            }
        })
    }
}
fn request(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new(format!("message {id}")).unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 100,
    }
}
fn actor() -> ActionContext {
    ActionContext::new("user", "test", "invoke").unwrap()
}
async fn invoke(agent: &Agent, id: &str) -> Result<ExecutionOutcome, AgentError> {
    tokio::time::timeout(Duration::from_secs(2), agent.invoke(request(id), actor()))
        .await
        .expect("agent invocation settled")
}

#[tokio::test]
async fn invocation_drains_and_persists_without_any_subscriber() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    for id in ["first", "second"] {
        assert_eq!(invoke(&agent, id).await, Ok(ExecutionOutcome::Completed));
    }
    let saved = storage.snapshot();
    assert_eq!(saved.invocations.len(), 2);
    for (record, id) in saved.invocations.iter().zip(["first", "second"]) {
        assert_eq!(record.request.execution_id.as_str(), id);
        assert_eq!(record.actor, actor());
        assert_eq!(record.result, Some(Ok(ExecutionOutcome::Completed)));
        assert_eq!(record.events.len(), 2);
        assert!(record
            .events
            .iter()
            .all(|event| event.execution_id().as_str() == id));
        assert!(matches!(
            record.events[1].update(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed)
        ));
    }
    assert_eq!(provider.calls.opens.lock().unwrap().as_slice(), &[None]);
    assert_eq!(
        agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations
            .len(),
        2
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn reconstruction_restores_provider_context_without_replaying_inputs() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    invoke(&agent, "first").await.unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let restored = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider.calls.opens.lock().unwrap().as_slice(),
        &[
            None,
            Some(ExecutionSessionId::new("provider-context").unwrap())
        ]
    );
    invoke(&restored, "second").await.unwrap();
    assert_eq!(storage.snapshot().invocations.len(), 2);
    restored.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn lease_and_provider_mismatch_fail_before_opening_another_context() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    assert!(matches!(
        SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(storage.clone())
        )
        .await,
        Err(StorageError::Busy)
    ));
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let mut other = TestProvider::new();
    let identity = &mut Arc::get_mut(&mut other).unwrap().identity;
    *identity =
        ProviderIdentity::new("another-provider", identity.model_id(), identity.context()).unwrap();
    assert!(matches!(
        attached_agent(other.clone(), storage.manager().await).await,
        Err(error) if matches!(error, AgentError::Storage(StorageError::IdentityMismatch))
    ));
    assert!(other.calls.opens.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_initial_save_prevents_opening_a_provider_context() {
    let storage = MemoryStorage::default();
    storage.fail_next();
    let provider = TestProvider::new();
    assert!(matches!(
        attached_agent(provider.clone(), storage.manager().await).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert!(provider.calls.opens.lock().unwrap().is_empty());
    assert!(provider.calls.closes.lock().unwrap().is_empty());
    assert!(!storage.0.lock().unwrap().leased);
}

#[tokio::test]
async fn failed_input_save_prevents_dispatch_and_preserves_previous_evidence() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    storage.fail_next();
    assert!(matches!(
        invoke(&agent, "unsaved").await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    assert!(storage.snapshot().invocations.is_empty());
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn failed_event_save_preserves_the_execution_result_and_reports_storage_failure() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    storage.0.lock().unwrap().fail_observation = Some("write-failure".into());
    let error = invoke(&agent, "write-failure").await.unwrap_err();
    assert!(
        matches!(error, AgentError::StorageAfterExecution { error: StorageError::Io(_), execution_result } if *execution_result == Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 1);
    assert!(!provider.calls.closes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn close_can_interrupt_an_active_invocation_from_a_clone() {
    let storage = MemoryStorage::default();
    let mut provider = TestProvider::new();
    Arc::get_mut(&mut provider).unwrap().wait_for_close = true;
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    let mut events = agent.subscribe();
    let running = agent.clone();
    let active = tokio::spawn(async move { running.invoke(request("waiting"), actor()).await });
    let observed = tokio::time::timeout(Duration::from_secs(2), events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Live text does not cause a write; close/settlement saves accumulated output.
    assert!(storage.snapshot().invocations[0].events.is_empty());
    tokio::time::timeout(Duration::from_secs(2), agent.close(close_action()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    assert_eq!(
        storage.snapshot().invocations[0].events.first(),
        Some(&observed)
    );
    assert_eq!(
        provider.calls.closes.lock().unwrap().first(),
        Some(&SessionCloseRequest::Explicit(close_action()))
    );
    assert_eq!(
        storage.snapshot().invocations[0].result,
        Some(Ok(ExecutionOutcome::Cancelled))
    );
}

struct OrderedHook {
    name: &'static str,
    calls: Arc<ProviderCalls>,
    before_error: bool,
    after_error: bool,
}
impl InvocationHook for OrderedHook {
    fn before_invocation(&self, _: &InvocationContext<'_>) -> Result<(), HookError> {
        self.calls
            .order
            .lock()
            .unwrap()
            .push(format!("before-{}", self.name));
        if self.before_error {
            Err(HookError::Failed(self.name.into()))
        } else {
            Ok(())
        }
    }
    fn after_invocation(
        &self,
        _: &InvocationContext<'_>,
        _: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        self.calls
            .order
            .lock()
            .unwrap()
            .push(format!("after-{}", self.name));
        if self.after_error {
            Err(HookError::Failed(self.name.into()))
        } else {
            Ok(())
        }
    }
}
#[tokio::test]
async fn hooks_run_in_registration_order_and_preserve_a_failed_provider_result() {
    let storage = MemoryStorage::default();
    let mut provider = TestProvider::new();
    Arc::get_mut(&mut provider).unwrap().outcome = Err(AgentError::Provider {
        code: -42,
        diagnostic: None,
    });
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    for name in ["one", "two"] {
        agent.add_invocation_hook(Arc::new(OrderedHook {
            name,
            calls: provider.calls.clone(),
            before_error: false,
            after_error: true,
        }));
    }
    assert!(
        matches!(invoke(&agent, "failure").await, Err(AgentError::AfterInvocationHooks { failures, execution_result }) if failures.len() == 2 && *execution_result == Err(AgentError::Provider { code: -42, diagnostic: None }))
    );
    assert_eq!(
        *provider.calls.order.lock().unwrap(),
        [
            "before-one",
            "before-two",
            "execute",
            "after-one",
            "after-two"
        ]
    );
    agent.close(close_action()).await.unwrap();
}
#[tokio::test]
async fn a_before_hook_failure_never_dispatches_or_runs_after_hooks() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    agent.add_invocation_hook(Arc::new(OrderedHook {
        name: "reject",
        calls: provider.calls.clone(),
        before_error: true,
        after_error: false,
    }));
    assert!(matches!(
        invoke(&agent, "rejected").await,
        Err(AgentError::BeforeInvocationHook(_))
    ));
    assert_eq!(*provider.calls.order.lock().unwrap(), ["before-reject"]);
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn unfinished_saved_invocations_are_retained_without_automatic_replay() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
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
            submission: SubmissionMode::Immediate,
            scheduling: Vec::new(),
            request: request("interrupted"),
            actor: actor(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            result: None,
        }],
    });
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        provider.calls.opens.lock().unwrap().as_slice(),
        &[Some(ExecutionSessionId::new("saved-context").unwrap())]
    );
    invoke(&agent, "new-input").await.unwrap();
    let saved = storage.snapshot();
    assert_eq!(
        saved.invocations[0].request.execution_id.as_str(),
        "interrupted"
    );
    assert_eq!(saved.invocations[0].result, None);
    assert_eq!(
        saved.invocations[1].request.execution_id.as_str(),
        "new-input"
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn failed_final_save_retains_the_original_provider_error() {
    let storage = MemoryStorage::default();
    let mut provider = TestProvider::new();
    Arc::get_mut(&mut provider).unwrap().outcome = Err(AgentError::Provider {
        code: -42,
        diagnostic: None,
    });
    let agent = attached_agent(provider, storage.manager().await)
        .await
        .unwrap();
    storage.0.lock().unwrap().fail_settlement = Some("provider-failure".into());
    assert!(
        matches!(invoke(&agent, "provider-failure").await, Err(AgentError::StorageAfterExecution { error: StorageError::Io(_), execution_result }) if *execution_result == Err(AgentError::Provider { code: -42, diagnostic: None }))
    );
}

#[tokio::test]
async fn unattached_manager_holds_lease_until_drop_without_writing() {
    let storage = MemoryStorage::default();
    let manager = storage.manager().await;
    assert!(manager.snapshot().await.is_none());
    assert_eq!(storage.0.lock().unwrap().writes, 0);
    assert!(matches!(
        SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(storage.clone())
        )
        .await,
        Err(StorageError::Busy)
    ));
    drop(manager);
    let reopened = storage.manager().await;
    assert!(reopened.snapshot().await.is_none());
    assert_eq!(storage.0.lock().unwrap().writes, 0);
}

#[tokio::test]
async fn restored_unresolved_admission_is_never_redispatched_by_retry() {
    for stage in [InvocationStage::Queued, InvocationStage::Running] {
        let storage = MemoryStorage::default();
        let provider = TestProvider::new();
        let mut scheduling = vec![InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(actor()),
        }];
        if stage == InvocationStage::Running {
            scheduling.push(InvocationSchedulingEvent {
                kind: InvocationKind::Queued,
                target: None,
                before: Some(InvocationStage::Queued),
                stage,
                cause: SchedulingCause::Dispatched,
                actor: None,
            });
        }
        let execution_id = ExecutionId::new("interrupted").unwrap();
        let mut queue_history = vec![QueueHistoryRecord {
            mutation: QueueMutation::Admitted {
                id: execution_id.clone(),
                kind: InvocationKind::Queued,
            },
            actor: Some(actor()),
            scheduling_length: Some(1),
        }];
        if stage == InvocationStage::Running {
            queue_history.push(QueueHistoryRecord {
                mutation: QueueMutation::Selected { id: execution_id },
                actor: None,
                scheduling_length: Some(1),
            });
        }
        storage.0.lock().unwrap().snapshot = Some(SessionSnapshot {
            queue_history,
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
                request: request("interrupted"),
                actor: actor(),
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling,
                result: None,
            }],
        });
        let agent = attached_agent(provider.clone(), storage.manager().await)
            .await
            .unwrap();
        assert!(matches!(
            agent.enqueue(request("interrupted"), actor()).await,
            Err(AgentError::SubmissionUnresolved)
        ));
        assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
        assert_eq!(storage.snapshot().invocations.len(), 1);
        agent.close(close_action()).await.unwrap();
    }
}
