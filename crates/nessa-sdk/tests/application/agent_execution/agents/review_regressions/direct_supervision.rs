//! Direct invocation ownership survives loss or suspension of its waiting caller.
use super::*;

#[tokio::test]
async fn never_polled_direct_invocation_has_no_effects() {
    let (agent, backend, storage) = probe(false).await;
    drop(agent.invoke(input("never-polled"), actor()));
    assert!(storage.snapshot().invocations.is_empty());
    assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn caller_loss_during_preparation_keeps_slot_and_boundary_steering() {
    let (agent, backend, storage) = probe(false).await;
    let (release, gate) = oneshot::channel();
    *backend.prepare_gate.lock().unwrap() = Some(gate);
    let caller = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("preparing"), actor()).await }
    });
    backend.preparing.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(
        agent.invoke(input("overlap"), actor()).await,
        Err(AgentError::Busy)
    );
    let SteeringDelivery::Queued(next) = agent.steer(input("correction"), actor()).await.unwrap()
    else {
        panic!("preparation must retain boundary steering")
    };
    release.send(()).unwrap();
    assert_eq!(next.wait().await, Ok(ExecutionOutcome::Completed));
    let saved = storage.snapshot();
    assert_eq!(saved.invocations.len(), 2);
    assert_eq!(
        saved.invocations[0].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
    assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn caller_loss_after_dispatch_keeps_native_target_and_settlement() {
    let (agent, backend, storage) = probe(false).await;
    let (release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let caller = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("active"), actor()).await }
    });
    backend.executing.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(
        agent.invoke(input("overlap"), actor()).await,
        Err(AgentError::Busy)
    );
    assert!(
        matches!(agent.steer(input("correction"), actor()).await.unwrap(),
        SteeringDelivery::Injected { target, .. } if target.as_str() == "active")
    );
    let next = agent.enqueue(input("next"), actor()).await.unwrap();
    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    assert_eq!(next.wait().await, Ok(ExecutionOutcome::Completed));
    let saved = storage.snapshot();
    assert_eq!(
        saved.invocations[0].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(backend.steers.load(Ordering::SeqCst), 1);
    assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn close_settles_direct_invocation_without_polling_its_waiter() {
    let storage = MemoryStorage::default();
    let mut provider = TestProvider::new();
    Arc::get_mut(&mut provider).unwrap().wait_for_close = true;
    let agent = attached_agent(provider, storage.manager().await)
        .await
        .unwrap();
    let mut events = agent.subscribe();
    let mut waiting = agent.invoke(input("suspended"), actor());
    poll_fn(|context| {
        assert!(waiting.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    events.next().await.unwrap().unwrap();
    timeout(Duration::from_secs(2), agent.close(actor()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(waiting.await, Ok(ExecutionOutcome::Cancelled));
    assert_eq!(
        storage.snapshot().invocations[0].result,
        Some(Ok(ExecutionOutcome::Cancelled))
    );
}

#[tokio::test]
async fn panicked_provider_retains_slot_until_cleanup_and_saves_failure() {
    let (agent, backend, storage) = probe(false).await;
    let (panic, execution_gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(execution_gate);
    let (release_cleanup, cleanup_gate) = oneshot::channel();
    *backend.close_gate.lock().unwrap() = Some(cleanup_gate);
    let caller = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("panicking"), actor()).await }
    });
    backend.executing.notified().await;
    // Probe deliberately unwraps its execution gate; losing this sender panics
    // inside the provider future after dispatch, outside the hook panic boundary.
    drop(panic);
    backend.closing.notified().await;
    assert_eq!(
        agent.invoke(input("overlap"), actor()).await,
        Err(AgentError::Busy)
    );
    assert!(!caller.is_finished());
    release_cleanup.send(()).unwrap();
    let result = caller.await.unwrap();
    assert!(matches!(result, Err(AgentError::Protocol(_))));
    assert_eq!(storage.snapshot().invocations[0].result, Some(result));
    assert!(matches!(
        backend.close_requests.lock().unwrap().as_slice(),
        [SessionCloseRequest::ExecutionFailed]
    ));
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn panicked_provider_with_uncertain_cleanup_keeps_admission_closed() {
    let (agent, backend, storage) = probe(false).await;
    backend.fail_close.store(true, Ordering::SeqCst);
    let (panic, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let caller = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("panicking"), actor()).await }
    });
    backend.executing.notified().await;
    drop(panic);
    let result = caller.await.unwrap();
    assert!(
        matches!(&result, Err(AgentError::OperationAndCleanupFailure { operation_error, cleanup_error })
        if matches!(operation_error.as_ref(), AgentError::Protocol(_)) && **cleanup_error == AgentError::CleanupUncertain)
    );
    assert_eq!(storage.snapshot().invocations[0].result, Some(result));
    assert_eq!(
        agent.invoke(input("blocked"), actor()).await,
        Err(AgentError::Closed)
    );
    assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
    backend.fail_close.store(false, Ordering::SeqCst);
    agent.close(actor()).await.unwrap();
}

struct PanicOnSettlementStorage(MemoryStorage);
struct PanicOnSettlementLease {
    backing: Box<dyn SessionStorageLease>,
    panicked: AtomicBool,
}
impl SessionStorage for PanicOnSettlementStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(PanicOnSettlementLease {
                backing: self.0.open(id).await?,
                panicked: AtomicBool::new(false),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for PanicOnSettlementLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            if snapshot
                .invocations
                .iter()
                .any(|record| record.result.is_some())
                && !self.panicked.swap(true, Ordering::SeqCst)
            {
                panic!("one-shot storage panic after result was observed");
            }
            self.backing.save(snapshot).await
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.backing.erase()
    }
}

#[tokio::test]
async fn settlement_storage_panic_preserves_already_observed_provider_success() {
    let storage = MemoryStorage::default();
    let manager = SessionManager::open(None, Arc::new(PanicOnSettlementStorage(storage.clone())))
        .await
        .unwrap();
    let agent = attached_agent(TestProvider::new(), manager).await.unwrap();
    let result = agent.invoke(input("settlement-panic"), actor()).await;
    assert!(
        matches!(&result, Err(AgentError::ExecutionObservation { error, execution_result: Some(outcome) })
        if matches!(error.as_ref(), AgentError::Protocol(_)) && **outcome == Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(storage.snapshot().invocations[0].result, Some(result));
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn rejected_direct_input_does_not_create_invocation_evidence() {
    let (agent, backend, storage) = probe(false).await;
    let mut request = input("invalid");
    request.reserved_output_tokens = u32::MAX;
    assert!(agent.invoke(request, actor()).await.is_err());
    assert!(storage.snapshot().invocations.is_empty());
    assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
    agent.close(actor()).await.unwrap();
}
