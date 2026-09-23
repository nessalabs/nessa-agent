//! Deterministic invocation admission, dispatch, steering, and cleanup interleavings.
//! Retry tests share these gated providers through the retries module.
use nessa_sdk::application::agent_execution::providers::{ProviderOpenFuture, ProviderOpenRequest};
mod retries;
use super::{agents::MemoryStorage, support::*};
use nessa_sdk::application::agent_execution::hooks::{
    HookError, InvocationContext, InvocationHook,
};
use std::{future::Future, time::Duration};
use tokio::sync::{mpsc, oneshot, watch, Notify};

struct Started {
    id: ExecutionId,
    release: oneshot::Sender<Result<ExecutionOutcome, AgentError>>,
}
struct GatedProvider {
    started: mpsc::UnboundedSender<Started>,
    steering: Result<SteeringOutcome, AgentError>,
    steered: Mutex<Vec<(ExecutionId, ExecutionRequest)>>,
    saved_before_steering: Mutex<Vec<InvocationRecord>>,
    storage: MemoryStorage,
    closed: AtomicUsize,
    steering_wait: Mutex<Option<oneshot::Receiver<()>>>,
    steering_started: Notify,
}
struct GatedBackend {
    provider: Arc<GatedProvider>,
    events: Mutex<Option<mpsc::UnboundedSender<ExecutionEvent>>>,
    closing: watch::Sender<bool>,
}
struct Events(mpsc::UnboundedReceiver<ExecutionEvent>);
impl ExecutionEventStream for Events {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async { Ok(self.0.recv().await) })
    }
}
struct GatedFactory(Arc<GatedProvider>);
impl AgentProvider for GatedFactory {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("gated", "fixture", "tests").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let (sender, receiver) = mpsc::unbounded_channel();
            let (closing, _) = watch::channel(false);
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("provider").unwrap(),
                    Arc::new(GatedBackend {
                        provider: self.0.clone(),
                        events: Mutex::new(Some(sender)),
                        closing,
                    }),
                    capabilities(),
                ),
                events: Box::new(Events(receiver)),
            })
        })
    }
}
impl ProviderSessionBackend for GatedBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> = {
                async move {
                    let (release, wait) = oneshot::channel();
                    self.provider
                        .started
                        .send(Started {
                            id: input.execution_id.clone(),
                            release,
                        })
                        .unwrap();
                    let mut closing = self.closing.subscribe();
                    let result = if *closing.borrow() {
                        Ok(ExecutionOutcome::Cancelled)
                    } else {
                        tokio::select! {
                            result = wait => result.unwrap_or(Err(AgentError::Closed)),
                            _ = closing.changed() => Ok(ExecutionOutcome::Cancelled),
                        }
                    };
                    if let Ok(outcome) = &result {
                        if let Some(sender) = self.events.lock().unwrap().as_ref() {
                            sender
                                .send(ExecutionEvent::new(
                                    input.execution_id,
                                    ExecutionUpdate::Finished(*outcome),
                                ))
                                .unwrap();
                        }
                    } else {
                        self.events.lock().unwrap().take();
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
    fn steer(
        &self,
        target: ExecutionId,
        input: ExecutionRequest,
    ) -> ProviderOperationFuture<'_, SteeringOutcome> {
        Box::pin(async move {
            let result: Result<SteeringOutcome, AgentError> = {
                async move {
                    let saved = self.provider.storage.snapshot();
                    let record = saved
                        .invocations
                        .iter()
                        .find(|record| record.request.execution_id == input.execution_id)
                        .expect("input persisted before provider steering");
                    self.provider
                        .saved_before_steering
                        .lock()
                        .unwrap()
                        .push(record.clone());
                    self.provider.steered.lock().unwrap().push((target, input));
                    let wait = self.provider.steering_wait.lock().unwrap().take();
                    self.provider.steering_started.notify_one();
                    if let Some(wait) = wait {
                        let _ = wait.await;
                    }
                    self.provider.steering.clone()
                }
            }
            .await;
            result
                .map_err(|error| ProviderOperationFailure::new(error, ProviderSessionState::Usable))
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
    fn close(&self, _: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            let result: Result<CloseOutcome, AgentError> = {
                async move {
                    self.provider.closed.fetch_add(1, Ordering::SeqCst);
                    self.closing.send_replace(true);
                    // Closing the stream lets execution settle even before its Finished observation.
                    self.events.lock().unwrap().take();
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
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("deterministic gate completed")
}
async fn fixture(
    steering: Result<SteeringOutcome, AgentError>,
) -> (
    Agent,
    MemoryStorage,
    Arc<GatedProvider>,
    mpsc::UnboundedReceiver<Started>,
) {
    let storage = MemoryStorage::default();
    let (started, receiver) = mpsc::unbounded_channel();
    let provider = Arc::new(GatedProvider {
        started,
        steering,
        steered: Mutex::new(Vec::new()),
        saved_before_steering: Mutex::new(Vec::new()),
        storage: storage.clone(),
        closed: AtomicUsize::new(0),
        steering_wait: Mutex::new(None),
        steering_started: Notify::new(),
    });
    let agent = attached_agent(
        Arc::new(GatedFactory(provider.clone())),
        storage.manager().await,
    )
    .await
    .unwrap();
    (agent, storage, provider, receiver)
}
fn request(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new(format!("input {id}")).unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 10,
    }
}
fn actor() -> ActionContext {
    ActionContext::new("user", "surface", "submission").unwrap()
}
async fn started(receiver: &mut mpsc::UnboundedReceiver<Started>, id: &str) -> Started {
    let started = within(receiver.recv()).await.unwrap();
    assert_eq!(started.id.as_str(), id);
    started
}
fn complete(started: Started) {
    started
        .release
        .send(Ok(ExecutionOutcome::Completed))
        .unwrap();
}
fn record(storage: &MemoryStorage, id: &str) -> InvocationRecord {
    storage
        .snapshot()
        .invocations
        .into_iter()
        .find(|record| record.request.execution_id.as_str() == id)
        .unwrap()
}
fn assert_lifecycle(record: &InvocationRecord, kind: InvocationKind) {
    assert_eq!(record.result, Some(Ok(ExecutionOutcome::Completed)));
    assert_eq!(
        record
            .scheduling
            .iter()
            .map(|event| (event.before, event.stage, event.cause))
            .collect::<Vec<_>>(),
        vec![
            (None, InvocationStage::Queued, SchedulingCause::Submitted),
            (
                Some(InvocationStage::Queued),
                InvocationStage::Running,
                SchedulingCause::Dispatched
            ),
            (
                Some(InvocationStage::Running),
                InvocationStage::Settled,
                SchedulingCause::ExecutionSettled
            ),
        ]
    );
    assert!(record.scheduling.iter().all(|event| event.kind == kind));
    assert_eq!(record.scheduling[0].actor, Some(actor()));
    assert!(record.scheduling[1..]
        .iter()
        .all(|event| event.actor.is_none()));
}

#[tokio::test]
async fn scheduling_fifo_across_clones_and_steering_priority_keep_saved_admission_order() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let first = agent.enqueue(request("first"), actor()).await.unwrap();
    let running = started(&mut calls, "first").await;
    assert_eq!(
        agent.invoke(request("busy"), actor()).await,
        Err(AgentError::Busy)
    );
    let clone = agent.clone();
    let ordinary_one = clone.enqueue(request("ordinary-1"), actor()).await.unwrap();
    let steering_one = agent
        .enqueue_steering(request("steering-1"), actor())
        .await
        .unwrap();
    let ordinary_two = agent.enqueue(request("ordinary-2"), actor()).await.unwrap();
    let steering_two = clone
        .enqueue_steering(request("steering-2"), actor())
        .await
        .unwrap();
    assert_eq!(ordinary_one.id().as_str(), "ordinary-1");
    assert_eq!(record(&storage, "ordinary-1").scheduling.len(), 1);
    assert_eq!(
        storage
            .snapshot()
            .invocations
            .iter()
            .map(|record| record.request.execution_id.as_str())
            .collect::<Vec<_>>(),
        [
            "first",
            "ordinary-1",
            "steering-1",
            "ordinary-2",
            "steering-2"
        ]
    );
    complete(running);
    assert_eq!(within(first.wait()).await, Ok(ExecutionOutcome::Completed));
    for (id, receipt, kind) in [
        ("steering-1", steering_one, InvocationKind::Steering),
        ("steering-2", steering_two, InvocationKind::Steering),
        ("ordinary-1", ordinary_one, InvocationKind::Queued),
        ("ordinary-2", ordinary_two, InvocationKind::Queued),
    ] {
        complete(started(&mut calls, id).await);
        assert_eq!(
            within(receipt.wait()).await,
            Ok(ExecutionOutcome::Completed)
        );
        assert_lifecycle(&record(&storage, id), kind);
    }
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_native_injection_saves_input_and_correlates_with_active_execution() {
    let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let first = agent.enqueue(request("active"), actor()).await.unwrap();
    let running = started(&mut calls, "active").await;
    let injected = agent.steer(request("correction"), actor()).await.unwrap();
    assert!(
        matches!(injected, SteeringDelivery::Injected { target, .. } if target.as_str() == "active")
    );
    assert_eq!(provider.steered.lock().unwrap()[0].0.as_str(), "active");
    let before = provider.saved_before_steering.lock().unwrap()[0].clone();
    assert_eq!(before.actor, actor());
    assert_eq!(
        before.request.user_message,
        request("correction").user_message
    );
    assert_eq!(before.scheduling.len(), 1);
    assert_eq!(before.target_event_offset, Some(0));
    let correction = record(&storage, "correction");
    assert_eq!(correction.target_event_offset, Some(0));
    assert_eq!(correction.result, None);
    assert!(correction.events.is_empty());
    let transition = &correction.scheduling[1];
    assert_eq!(transition.target.as_ref().unwrap().as_str(), "active");
    assert_eq!(transition.before, Some(InvocationStage::Queued));
    assert_eq!(transition.stage, InvocationStage::Injected);
    assert_eq!(transition.cause, SchedulingCause::SteeringInjected);
    assert_eq!(transition.actor, None);
    complete(running);
    within(first.wait()).await.unwrap();
    assert!(calls.try_recv().is_err());
    assert_eq!(
        record(&storage, "active").events[0].execution_id().as_str(),
        "active"
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_unsupported_and_ambiguous_steering_are_never_replayed() {
    for error in [
        AgentError::Unsupported("native steering".into()),
        AgentError::Deadline,
        AgentError::Transport("uncertain reply".into()),
    ] {
        let (agent, storage, provider, mut calls) = fixture(Err(error.clone())).await;
        let first = agent.enqueue(request("active"), actor()).await.unwrap();
        let running = started(&mut calls, "active").await;
        let pending = agent.enqueue(request("pending"), actor()).await.unwrap();
        assert!(
            matches!(agent.steer(request("correction"), actor()).await, Err(actual) if actual == error)
        );
        assert_eq!(provider.steered.lock().unwrap().len(), 1);
        assert_eq!(
            record(&storage, "correction").result,
            Some(Err(error.clone()))
        );
        complete(running);
        within(first.wait()).await.unwrap();
        // Every rejection in this fixture explicitly reports a usable provider.
        // Diagnostic wording cannot cancel unrelated waiting input.
        complete(started(&mut calls, "pending").await);
        within(pending.wait()).await.unwrap();
        // A later explicit receipt proves no ambiguous input was replayed.
        let next = agent.enqueue(request("explicit"), actor()).await.unwrap();
        complete(started(&mut calls, "explicit").await);
        within(next.wait()).await.unwrap();
        assert!(calls.try_recv().is_err());
        agent.close(close_action()).await.unwrap();
    }
}

#[tokio::test]
async fn scheduling_idle_and_prompt_required_steering_run_at_next_boundary() {
    let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::PromptRequired)).await;
    let SteeringDelivery::Queued(first) = agent.steer(request("idle"), actor()).await.unwrap()
    else {
        panic!("idle input must queue")
    };
    let running = started(&mut calls, "idle").await;
    assert!(provider.steered.lock().unwrap().is_empty());
    let ordinary = agent.enqueue(request("ordinary"), actor()).await.unwrap();
    let SteeringDelivery::Queued(correction) =
        agent.steer(request("correction"), actor()).await.unwrap()
    else {
        panic!("unconsumed input must queue")
    };
    complete(running);
    within(first.wait()).await.unwrap();
    complete(started(&mut calls, "correction").await);
    within(correction.wait()).await.unwrap();
    assert_lifecycle(&record(&storage, "correction"), InvocationKind::Steering);
    assert!(record(&storage, "correction")
        .scheduling
        .iter()
        .all(|event| event.target.as_ref().unwrap().as_str() == "idle"));
    complete(started(&mut calls, "ordinary").await);
    within(ordinary.wait()).await.unwrap();
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_close_cancels_pending_with_attribution_without_dispatching_it() {
    let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let _running = started(&mut calls, "active").await;
    let ordinary = agent.enqueue(request("ordinary"), actor()).await.unwrap();
    let steering = agent
        .enqueue_steering(request("steering"), actor())
        .await
        .unwrap();
    within(agent.clone().close(close_action())).await.unwrap();
    assert_eq!(within(active.wait()).await, Ok(ExecutionOutcome::Cancelled));
    for (id, ticket) in [("ordinary", ordinary), ("steering", steering)] {
        assert_eq!(within(ticket.wait()).await, Err(AgentError::Closed));
        let saved = record(&storage, id);
        assert_eq!(saved.result, None);
        assert_eq!(saved.scheduling.len(), 2);
        let event = &saved.scheduling[1];
        assert_eq!(event.before, Some(InvocationStage::Queued));
        assert_eq!(event.stage, InvocationStage::Cancelled);
        assert_eq!(event.cause, SchedulingCause::SessionClosed);
        assert_eq!(event.actor, Some(close_action()));
    }
    assert_eq!(provider.closed.load(Ordering::SeqCst), 1);
    assert!(calls.try_recv().is_err());
}

#[tokio::test]
async fn scheduling_dropped_receipt_and_agent_clone_do_not_abandon_execution_or_evidence() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let clone = agent.clone();
    let dropped = clone.enqueue(request("dropped"), actor()).await.unwrap();
    let running = started(&mut calls, "dropped").await;
    drop(dropped);
    drop(clone);
    let barrier = agent.enqueue(request("barrier"), actor()).await.unwrap();
    complete(running);
    complete(started(&mut calls, "barrier").await);
    within(barrier.wait()).await.unwrap();
    assert_lifecycle(&record(&storage, "dropped"), InvocationKind::Queued);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_failed_admission_save_allows_retry_without_losing_identity() {
    let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    storage.fail_next();
    assert!(matches!(
        agent.enqueue(request("retry"), actor()).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert!(storage.snapshot().invocations.is_empty());
    assert!(calls.try_recv().is_err());
    let retry = agent.enqueue(request("retry"), actor()).await.unwrap();
    let running = started(&mut calls, "retry").await;
    storage.fail_next();
    assert!(matches!(
        agent.steer(request("unsaved-steering"), actor()).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert!(provider.steered.lock().unwrap().is_empty());
    complete(running);
    within(retry.wait()).await.unwrap();
    assert_eq!(storage.snapshot().invocations.len(), 1);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_provider_failure_cancels_remainder_with_automatic_cause() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let running = started(&mut calls, "active").await;
    let pending = agent.enqueue(request("pending"), actor()).await.unwrap();
    running
        .release
        .send(Err(AgentError::Transport("broken".into())))
        .unwrap();
    assert_eq!(
        within(active.wait()).await,
        Err(AgentError::Transport("broken".into()))
    );
    assert_eq!(within(pending.wait()).await, Err(AgentError::Closed));
    let pending = record(&storage, "pending");
    let transition = pending.scheduling.last().unwrap();
    assert_eq!(transition.stage, InvocationStage::Cancelled);
    assert_eq!(transition.cause, SchedulingCause::RunnerStopped);
    assert_eq!(transition.actor, None);
    assert!(calls.try_recv().is_err());
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_capacity_counts_only_pending_and_retry_preserves_evidence() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let _running = started(&mut calls, "active").await;
    let mut pending = Vec::new();
    for index in 0..64 {
        pending.push(
            agent
                .enqueue(request(&format!("pending-{index}")), actor())
                .await
                .unwrap(),
        );
    }
    assert!(matches!(
        agent.enqueue(request("overflow"), actor()).await,
        Err(AgentError::Scheduling(SchedulingError::Full))
    ));
    let retried = agent.enqueue(request("pending-0"), actor()).await.unwrap();
    assert_eq!(retried.id().as_str(), "pending-0");
    assert_eq!(storage.snapshot().invocations.len(), 65);
    within(agent.close(close_action())).await.unwrap();
    assert_eq!(within(active.wait()).await, Ok(ExecutionOutcome::Cancelled));
    for receipt in pending {
        assert_eq!(within(receipt.wait()).await, Err(AgentError::Closed));
    }
    assert!(calls.try_recv().is_err());
}

#[tokio::test]
async fn scheduling_failed_dispatch_save_retains_attempt_and_stops_remaining_work() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let running = started(&mut calls, "active").await;
    let failed = agent.enqueue(request("failed"), actor()).await.unwrap();
    let pending = agent.enqueue(request("pending"), actor()).await.unwrap();
    storage.fail_scheduling("failed", InvocationStage::Running);
    complete(running);
    within(active.wait()).await.unwrap();
    assert!(matches!(
        within(failed.wait()).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(within(pending.wait()).await, Err(AgentError::Closed));
    let saved = record(&storage, "failed");
    assert!(matches!(
        saved.result,
        Some(Err(AgentError::Storage(StorageError::Io(_))))
    ));
    assert_eq!(saved.scheduling[1].stage, InvocationStage::Running);
    assert_eq!(saved.scheduling[2].before, Some(InvocationStage::Running));
    assert_eq!(saved.scheduling[2].stage, InvocationStage::Settled);
    assert_eq!(
        record(&storage, "pending").scheduling.last().unwrap().cause,
        SchedulingCause::RunnerStopped
    );
    assert!(calls.try_recv().is_err());
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_failed_close_audit_reports_failure_and_still_cleans_every_pending_item() {
    let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let _running = started(&mut calls, "active").await;
    let first = agent.enqueue(request("first"), actor()).await.unwrap();
    let second = agent.enqueue(request("second"), actor()).await.unwrap();
    storage.fail_scheduling("first", InvocationStage::Cancelled);
    assert!(
        matches!(within(agent.close(close_action())).await, Err(AgentError::StorageDuringClose { error: StorageError::Io(_), cleanup_result }) if cleanup_result.is_ok())
    );
    assert!(matches!(
        within(first.wait()).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(within(second.wait()).await, Err(AgentError::Closed));
    assert_eq!(within(active.wait()).await, Ok(ExecutionOutcome::Cancelled));
    assert_eq!(provider.closed.load(Ordering::SeqCst), 1);
    for id in ["first", "second"] {
        let saved = record(&storage, id);
        assert_eq!(
            saved.scheduling.last().unwrap().cause,
            SchedulingCause::SessionClosed
        );
        assert_eq!(saved.scheduling.last().unwrap().actor, Some(close_action()));
    }
    assert!(calls.try_recv().is_err());
}

#[tokio::test]
async fn scheduling_runner_cleanup_audit_failure_is_visible_without_losing_other_cancellations() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let running = started(&mut calls, "active").await;
    let first = agent.enqueue(request("first"), actor()).await.unwrap();
    let second = agent.enqueue(request("second"), actor()).await.unwrap();
    storage.fail_scheduling("first", InvocationStage::Cancelled);
    running
        .release
        .send(Err(AgentError::Transport("failed".into())))
        .unwrap();
    assert!(
        matches!(within(active.wait()).await, Err(AgentError::StorageAfterExecution { error: StorageError::Io(_), execution_result }) if *execution_result == Err(AgentError::Transport("failed".into())))
    );
    assert!(matches!(
        within(first.wait()).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(within(second.wait()).await, Err(AgentError::Closed));
    for id in ["first", "second"] {
        let saved = record(&storage, id);
        assert_eq!(
            saved.scheduling.last().unwrap().cause,
            SchedulingCause::RunnerStopped
        );
        assert_eq!(saved.scheduling.last().unwrap().actor, None);
    }
    assert!(calls.try_recv().is_err());
    agent.close(close_action()).await.unwrap();
}

struct ObservedAfter(Mutex<Vec<Result<ExecutionOutcome, AgentError>>>);
impl InvocationHook for ObservedAfter {
    fn after_invocation(
        &self,
        _: &InvocationContext<'_>,
        result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        self.0.lock().unwrap().push(result.clone());
        Ok(())
    }
}

#[tokio::test]
async fn scheduling_close_retains_provider_settlement_and_calls_after_hook_once() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let hook = Arc::new(ObservedAfter(Mutex::new(Vec::new())));
    agent.add_invocation_hook(hook.clone());
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let _running = started(&mut calls, "active").await;
    within(agent.close(close_action())).await.unwrap();
    assert_eq!(within(active.wait()).await, Ok(ExecutionOutcome::Cancelled));
    assert_eq!(*hook.0.lock().unwrap(), [Ok(ExecutionOutcome::Cancelled)]);
    assert_eq!(
        record(&storage, "active").result,
        Some(Ok(ExecutionOutcome::Cancelled))
    );
}

#[tokio::test]
async fn scheduling_close_interrupts_stalled_native_steering_and_retains_caller_cause() {
    let (agent, storage, provider, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let _running = started(&mut calls, "active").await;
    let (held, wait) = oneshot::channel();
    *provider.steering_wait.lock().unwrap() = Some(wait);
    let steering_agent = agent.clone();
    let steering =
        tokio::spawn(async move { steering_agent.steer(request("stalled"), actor()).await });
    within(provider.steering_started.notified()).await;
    let pending = record(&storage, "stalled");
    assert_eq!(pending.scheduling.len(), 1);
    within(agent.close(close_action())).await.unwrap();
    assert!(matches!(
        within(steering).await.unwrap(),
        Err(AgentError::Closed)
    ));
    assert_eq!(within(active.wait()).await, Ok(ExecutionOutcome::Cancelled));
    let saved = record(&storage, "stalled");
    assert_eq!(saved.result, Some(Err(AgentError::Closed)));
    let event = saved.scheduling.last().unwrap();
    assert_eq!(event.before, Some(InvocationStage::Queued));
    assert_eq!(event.stage, InvocationStage::Cancelled);
    assert_eq!(event.cause, SchedulingCause::SessionClosed);
    assert_eq!(event.actor, Some(close_action()));
    assert_eq!(event.target.as_ref().unwrap().as_str(), "active");
    assert!(calls.try_recv().is_err());
    drop(held);
}

#[tokio::test]
async fn scheduling_withdraws_ordinary_and_priority_work_once_with_verified_attribution() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let running = started(&mut calls, "active").await;
    let ordinary = agent.enqueue(request("ordinary"), actor()).await.unwrap();
    let steering = agent
        .enqueue_steering(request("steering"), actor())
        .await
        .unwrap();
    let retained = agent.enqueue(request("retained"), actor()).await.unwrap();
    let withdraw = ActionContext::new("owner", "surface", "withdraw-command").unwrap();
    assert_eq!(
        agent
            .remove_queued(request("active").execution_id, withdraw.clone())
            .await
            .unwrap(),
        QueueRemoval::NotPending
    );
    for (id, ticket, kind) in [
        ("ordinary", ordinary, InvocationKind::Queued),
        ("steering", steering, InvocationKind::Steering),
    ] {
        assert_eq!(
            agent
                .clone()
                .remove_queued(request(id).execution_id, withdraw.clone())
                .await
                .unwrap(),
            QueueRemoval::Removed
        );
        assert_eq!(within(ticket.wait()).await, Err(AgentError::Closed));
        assert_eq!(
            agent
                .remove_queued(request(id).execution_id, actor())
                .await
                .unwrap(),
            QueueRemoval::NotPending
        );
        let saved = record(&storage, id);
        assert_eq!(saved.scheduling.len(), 2);
        let removed = &saved.scheduling[1];
        assert_eq!(removed.kind, kind);
        assert_eq!(removed.before, Some(InvocationStage::Queued));
        assert_eq!(removed.stage, InvocationStage::Cancelled);
        assert_eq!(removed.cause, SchedulingCause::Withdrawn);
        assert_eq!(removed.actor, Some(withdraw.clone()));
        let retry = match kind {
            InvocationKind::Queued => agent.enqueue(request(id), actor()).await,
            InvocationKind::Steering => agent.enqueue_steering(request(id), actor()).await,
        }
        .unwrap();
        assert_eq!(within(retry.wait()).await, Err(AgentError::Closed));
    }
    complete(running);
    within(active.wait()).await.unwrap();
    complete(started(&mut calls, "retained").await);
    within(retained.wait()).await.unwrap();
    assert!(calls.try_recv().is_err());
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_injected_and_unknown_inputs_cannot_be_unsent() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let running = started(&mut calls, "active").await;
    assert!(matches!(
        agent.steer(request("injected"), actor()).await.unwrap(),
        SteeringDelivery::Injected { .. }
    ));
    for id in ["injected", "unknown"] {
        assert_eq!(
            agent
                .remove_queued(request(id).execution_id, actor())
                .await
                .unwrap(),
            QueueRemoval::NotPending
        );
    }
    assert_eq!(record(&storage, "injected").scheduling.len(), 2);
    assert_eq!(
        record(&storage, "injected")
            .scheduling
            .last()
            .unwrap()
            .stage,
        InvocationStage::Injected
    );
    complete(running);
    within(active.wait()).await.unwrap();
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn scheduling_withdrawal_audit_failure_is_visible_but_removed_work_stays_removed() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let running = started(&mut calls, "active").await;
    let removed = agent.enqueue(request("removed"), actor()).await.unwrap();
    storage.fail_scheduling("removed", InvocationStage::Cancelled);
    assert!(matches!(
        agent
            .remove_queued(request("removed").execution_id, actor())
            .await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert!(matches!(
        within(removed.wait()).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(
        agent
            .remove_queued(request("removed").execution_id, close_action())
            .await
            .unwrap(),
        QueueRemoval::NotPending
    );
    let retry = agent.enqueue(request("removed"), actor()).await.unwrap();
    assert!(matches!(
        within(retry.wait()).await,
        Err(AgentError::Storage(StorageError::Io(_)))
    ));
    complete(running);
    within(active.wait()).await.unwrap();
    let saved = record(&storage, "removed");
    assert_eq!(saved.scheduling.len(), 2);
    assert_eq!(saved.scheduling[1].cause, SchedulingCause::Withdrawn);
    assert_eq!(saved.scheduling[1].actor, Some(actor()));
    assert!(calls.try_recv().is_err());
    agent.close(close_action()).await.unwrap();
}

struct FailingAfter(AtomicUsize);
impl InvocationHook for FailingAfter {
    fn after_invocation(
        &self,
        _: &InvocationContext<'_>,
        _: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(HookError::Failed("after-close".into()))
    }
}

#[tokio::test]
async fn scheduling_close_cause_survives_after_hook_failure_without_relabelling_cancellation() {
    let (agent, storage, _, mut calls) = fixture(Ok(SteeringOutcome::Injected)).await;
    let hook = Arc::new(FailingAfter(AtomicUsize::new(0)));
    agent.add_invocation_hook(hook.clone());
    let active = agent.enqueue(request("active"), actor()).await.unwrap();
    let _running = started(&mut calls, "active").await;
    within(agent.close(close_action())).await.unwrap();
    assert!(
        matches!(within(active.wait()).await, Err(AgentError::AfterInvocationHooks { failures, execution_result }) if failures.len() == 1 && *execution_result == Ok(ExecutionOutcome::Cancelled))
    );
    assert_eq!(hook.0.load(Ordering::SeqCst), 1);
    let saved = record(&storage, "active");
    let transition = saved.scheduling.last().unwrap();
    assert_eq!(transition.stage, InvocationStage::Cancelled);
    assert_eq!(transition.cause, SchedulingCause::SessionClosed);
    assert_eq!(transition.actor, Some(close_action()));
}
