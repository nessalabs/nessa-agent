//! A refusal is acknowledged before another worker's placement can block.
use super::*;
use crate::conversation::application::{ConversationBindings, WorkerBinding};
use crate::conversation::infrastructure::FileBindings;

struct PausedAfterRefusal {
    inner: FileBindings,
    worker: Mutex<Option<Arc<PoolWorker>>>,
    pause: AtomicBool,
    entered: Notify,
    release: Notify,
}
impl ConversationBindings for PausedAfterRefusal {
    fn bind<'a>(
        &'a self,
        id: &'a ConversationId,
        binding: &'a WorkerBinding,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        self.inner.bind(id, binding)
    }
    fn binding<'a>(
        &'a self,
        id: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<Option<WorkerBinding>, PlacementError>> {
        Box::pin(async move {
            let refused = self
                .worker
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|worker| worker.draining.load(Ordering::SeqCst));
            if refused && self.pause.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.inner.binding(id).await
        })
    }
    fn erase<'a>(
        &'a self,
        id: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        self.inner.erase(id)
    }
}

async fn acknowledged_before_next_placement(fail_once: bool) {
    let root = tempfile::tempdir().unwrap();
    let bindings = Arc::new(PausedAfterRefusal {
        inner: FileBindings::new(root.path().join("bindings")).unwrap(),
        worker: Mutex::default(),
        pause: AtomicBool::new(true),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let (harness, [w1, w2]) = pool_harness_with(root.path(), bindings.clone());
    *bindings.worker.lock().unwrap() = Some(w1.clone());
    harness.create_on(super::super::POOL).await.unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    *w1.refuse.lock().unwrap() = Some(LeaseRefusal::EnvironmentUnreachable);
    w1.drains.store(true, Ordering::SeqCst);
    harness
        .storage
        .fail_issue
        .store(fail_once, Ordering::SeqCst);
    let mut opening = tokio::spawn({
        let service = harness.service.clone();
        let id = harness.id.clone();
        async move {
            service
                .create(
                    id,
                    caller("draining-read"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            _ = bindings.entered.notified() => {},
            result = &mut opening => panic!("opening ended before placement pause: {result:?}; asks={:?}", (w1.asked(), w2.asked())),
        }
    }).await.unwrap();
    // Read through the actual RecordStorage, independently of the writer and
    // before another placement returns or Agent::prepare is reached.
    let committed = harness
        .storage
        .inner
        .read_committed(SessionId::new(harness.id.to_string()).unwrap())
        .await
        .unwrap()
        .unwrap();
    let lease = committed.snapshot().unwrap().lease.as_ref().unwrap();
    let refusals: Vec<_> = lease
        .records()
        .iter()
        .filter_map(|record| match record {
            LeaseRecord::Refused {
                lease,
                terms,
                refusal,
                actor,
                ..
            } => Some((lease, terms, refusal, actor)),
            _ => None,
        })
        .collect();
    assert_eq!(refusals.len(), 1);
    let (_, terms, refusal, actor) = refusals[0];
    assert_eq!(terms.environment, EnvironmentRef::Peer(w1.peer));
    assert_eq!(*refusal, LeaseRefusal::EnvironmentUnreachable);
    assert_eq!(*actor, caller("draining-read").actor().unwrap());
    assert_eq!(w2.asked(), 0);
    bindings.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("final-close"))
        .await
        .unwrap();
    use nessa_sdk::application::agent_execution::sessions::{
        CommittedEventData, EventRead, EventReadLimits, EventStart,
    };
    let mut history = harness
        .storage
        .inner
        .events(
            SessionId::new(harness.id.to_string()).unwrap(),
            EventStart::Beginning,
        )
        .await
        .unwrap();
    let mut saved_refusals = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match history
                .read(EventReadLimits::new(64, 1024 * 1024).unwrap())
                .await
                .unwrap()
            {
                EventRead::Preparing => {}
                EventRead::Page(page) => {
                    for event in page.events() {
                        match event.data() {
                            CommittedEventData::Change(SessionChange::Lease(
                                record @ LeaseRecord::Refused { .. },
                            )) => saved_refusals.push(record.clone()),
                            CommittedEventData::Change(_) => {}
                            CommittedEventData::Gap(_) => {
                                panic!("the fixture history must be fully readable")
                            }
                        }
                    }
                }
                EventRead::Idle(_) => break,
            }
        }
    })
    .await
    .unwrap();
    // Replay the complete persisted history, including the refusal superseded
    // by workerB's issuance. CurrentLease intentionally retains only B.
    let expected = lease
        .records()
        .iter()
        .find(|record| matches!(record, LeaseRecord::Refused { .. }))
        .unwrap();
    assert_eq!(saved_refusals, vec![expected.clone()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_draining_refusal_is_durable_before_the_next_placement_wait() {
    acknowledged_before_next_placement(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_draining_refusal_retries_its_retained_failed_save_before_placement() {
    acknowledged_before_next_placement(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unacknowledged_draining_refusal_stops_placement_with_storage_failure() {
    let root = tempfile::tempdir().unwrap();
    let (harness, [w1, w2], _) = pool_harness(root.path());
    harness.create_on(super::super::POOL).await.unwrap();
    harness
        .service
        .close(harness.id.clone(), caller("close"))
        .await
        .unwrap();
    *w1.refuse.lock().unwrap() = Some(LeaseRefusal::EnvironmentUnreachable);
    w1.drains.store(true, Ordering::SeqCst);
    *harness.storage.failing.lock().unwrap() =
        Some(|record| matches!(record, LeaseRecord::Refused { .. }));
    let result = harness
        .service
        .create(
            harness.id.clone(),
            caller("draining-read"),
            RequestedConversation::default(),
        )
        .await;
    assert!(
        matches!(result, Err(ConversationError::Storage(_))),
        "{result:?}; asks={:?}",
        (w1.asked(), w2.asked())
    );
    assert_eq!(w2.asked(), 0);
    *harness.storage.failing.lock().unwrap() = None;
}
