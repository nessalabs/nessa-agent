//! Public close generations, exact debt and durable Completion boundaries.
use super::*;
use nessa_sdk::application::agent_execution::subagents::LiveRoom;
use nessa_sdk::domain::agent_execution::subagents::{CloseEvidenceDetail, SettlementProof};
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::AtomicBool,
    task::{Context, Waker},
};

struct QueuedResources {
    reports: Mutex<VecDeque<ResourceReport>>,
    closes: AtomicUsize,
    causes: Mutex<Vec<(LifetimeCause, Initiator)>>,
}

#[async_trait]
impl ChildResources for QueuedResources {
    async fn close(&self, cause: &LifetimeCause, initiator: &Initiator) -> ResourceReport {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.causes
            .lock()
            .unwrap()
            .push((cause.clone(), initiator.clone()));
        self.reports
            .lock()
            .unwrap()
            .pop_front()
            .expect("only planned physical cleanup may run")
    }
}

struct QueuedFactory {
    resources: Arc<QueuedResources>,
    submits: Arc<AtomicUsize>,
}

#[async_trait]
impl ChildFactory for QueuedFactory {
    async fn prepare(&self, _: PrepareRequest) -> Result<PreparedChild, PrepareFailure> {
        Ok(PreparedChild {
            resources: self.resources.clone(),
            submit: Arc::new(ScriptSubmit {
                submits: self.submits.clone(),
                result: Ok(TaskReceiptId::new("settlement-receipt").unwrap()),
            }),
        })
    }
}

struct ObservationGate {
    entered: Notify,
    release: Notify,
}

struct DebtAudit {
    records: Mutex<Vec<OwnershipEvidence>>,
    target: Mutex<Option<AgentLifetimeId>>,
    reject_failed: AtomicUsize,
    gate: Mutex<Option<Arc<ObservationGate>>>,
}

impl DebtAudit {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(Vec::new()),
            target: Mutex::new(None),
            reject_failed: AtomicUsize::new(0),
            gate: Mutex::new(None),
        })
    }

    fn observations(&self, target: &AgentLifetimeId) -> Vec<OwnershipEvidence> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                record.child_lifetime.as_ref() == Some(target)
                    && matches!(
                        record.close_detail,
                        Some(CloseEvidenceDetail::ResourceObservation { .. })
                    )
            })
            .cloned()
            .collect()
    }

    fn completions(&self) -> usize {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.close_detail == Some(CloseEvidenceDetail::Completion))
            .count()
    }

    fn hold_released(&self, target: &AgentLifetimeId) -> Arc<ObservationGate> {
        *self.target.lock().unwrap() = Some(target.clone());
        let gate = Arc::new(ObservationGate {
            entered: Notify::new(),
            release: Notify::new(),
        });
        *self.gate.lock().unwrap() = Some(gate.clone());
        gate
    }
}

#[async_trait]
impl OwnershipAudit for DebtAudit {
    async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
        self.records.lock().unwrap().push(evidence.clone());
        let selected = self.target.lock().unwrap().as_ref() == evidence.child_lifetime.as_ref();
        let gate = if selected
            && matches!(
                evidence.close_detail,
                Some(CloseEvidenceDetail::ResourceObservation {
                    physical: PhysicalFact::Released,
                    ..
                })
            ) {
            self.gate.lock().unwrap().take()
        } else {
            None
        };
        if let Some(gate) = gate {
            gate.entered.notify_one();
            gate.release.notified().await;
            return Err(PortFailure::Rejected);
        }
        if selected
            && matches!(
                evidence.close_detail,
                Some(CloseEvidenceDetail::ResourceObservation {
                    physical: PhysicalFact::Failed,
                    ..
                })
            )
            && take_rejection(&self.reject_failed)
        {
            return Err(PortFailure::Rejected);
        }
        Ok(())
    }
}

fn take_rejection(counter: &AtomicUsize) -> bool {
    let mut remaining = counter.load(Ordering::SeqCst);
    while remaining != 0 {
        match counter.compare_exchange(remaining, remaining - 1, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => return true,
            Err(actual) => remaining = actual,
        }
    }
    false
}

struct PhaseStore {
    memory: Arc<MemoryOwnershipStore>,
    reject_safety: AtomicBool,
    reject_final: AtomicBool,
    safety_rejections: AtomicUsize,
    final_rejections: AtomicUsize,
    watch_intent: Mutex<Option<AgentLifetimeId>>,
    intent_saved: Notify,
}

impl PhaseStore {
    fn new(memory: Arc<MemoryOwnershipStore>) -> Arc<Self> {
        Arc::new(Self {
            memory,
            reject_safety: AtomicBool::new(false),
            reject_final: AtomicBool::new(false),
            safety_rejections: AtomicUsize::new(0),
            final_rejections: AtomicUsize::new(0),
            watch_intent: Mutex::new(None),
            intent_saved: Notify::new(),
        })
    }
}

#[async_trait]
impl OwnershipStore for PhaseStore {
    async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {
        let safety = snapshot.lifetimes.iter().any(|row| {
            row.state == LifetimeState::Closing
                && snapshot
                    .settlements
                    .iter()
                    .any(|settlement| settlement.close_lifetime == row.lifetime_id)
        });
        if safety && self.reject_safety.swap(false, Ordering::SeqCst) {
            self.safety_rejections.fetch_add(1, Ordering::SeqCst);
            return Err(PortFailure::Rejected);
        }
        if snapshot
            .lifetimes
            .iter()
            .any(|row| row.state == LifetimeState::Closed)
            && self.reject_final.swap(false, Ordering::SeqCst)
        {
            self.final_rejections.fetch_add(1, Ordering::SeqCst);
            return Err(PortFailure::Rejected);
        }
        self.memory.write(snapshot).await?;
        let notify = {
            let mut watched = self.watch_intent.lock().unwrap();
            if watched.as_ref().is_some_and(|id| {
                snapshot
                    .lifetimes
                    .iter()
                    .any(|row| &row.lifetime_id == id && row.state == LifetimeState::Closing)
            }) {
                watched.take();
                true
            } else {
                false
            }
        };
        if notify {
            self.intent_saved.notify_one();
        }
        Ok(())
    }

    async fn read(&self) -> Result<OwnershipSnapshot, PortFailure> {
        self.memory.read().await
    }
}

fn settlement_world(
    reports: Vec<ResourceReport>,
) -> (
    World,
    Arc<QueuedResources>,
    Arc<DebtAudit>,
    Arc<PhaseStore>,
    Arc<LiveCapacity>,
) {
    let mut world = World::new(1);
    let resources = Arc::new(QueuedResources {
        reports: Mutex::new(reports.into()),
        closes: AtomicUsize::new(0),
        causes: Mutex::new(Vec::new()),
    });
    let audit = DebtAudit::new();
    let store = PhaseStore::new(world.store.clone());
    let room = Arc::new(LiveCapacity::new(1));
    world.coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: store.clone(),
        audit: audit.clone(),
        factory: Arc::new(QueuedFactory {
            resources: resources.clone(),
            submits: Arc::new(AtomicUsize::new(0)),
        }),
        room: room.clone(),
    });
    (world, resources, audit, store, room)
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("close generation must terminate")
}

fn poll_pending(future: Pin<&mut impl Future>) {
    assert!(future
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
}

async fn close_with(
    coordinator: &OwnershipCoordinator,
    id: &AgentLifetimeId,
    cause: LifetimeCause,
    initiator: Initiator,
) -> Result<(), OwnershipFailure> {
    coordinator
        .end_lifetime(CloseCommand {
            lifetime: id.clone(),
            cause,
            initiator,
            external_attachment: false,
            timeout: None,
        })
        .await
}

#[tokio::test]
async fn failed_observation_debt_survives_release_and_is_retried_once_per_generation() {
    let (world, resources, audit, _, room) = settlement_world(vec![
        ResourceReport {
            physical: PhysicalFact::Failed,
            evidence: EvidenceFact::Acknowledged,
        },
        released(),
    ]);
    let root = bounded(world.root()).await;
    let child = bounded(
        world
            .coordinator
            .spawn(world.command(&root, "mixed-debt", "task")),
    )
    .await
    .unwrap()
    .child;
    *audit.target.lock().unwrap() = Some(child.clone());
    audit.reject_failed.store(2, Ordering::SeqCst);
    assert_eq!(
        bounded(world.close(&child)).await,
        Err(OwnershipFailure::Incomplete)
    );
    let first = audit.observations(&child);
    assert_eq!(first.len(), 1);
    assert!(!room.try_reserve());
    assert_eq!(
        bounded(world.close(&child)).await,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    let second = audit.observations(&child);
    assert_eq!(second.len(), 3);
    assert_eq!(
        second[0], second[1],
        "Failed debt must retain its exact first record"
    );
    assert!(matches!(
        second[2].close_detail,
        Some(CloseEvidenceDetail::ResourceObservation {
            physical: PhysicalFact::Released,
            ..
        })
    ));
    assert_eq!(resources.closes.load(Ordering::SeqCst), 2);
    assert!(room.try_reserve());
    room.release();
    let snapshot = bounded(world.store.read()).await.unwrap();
    let restored = OwnershipGraph::restore(snapshot.clone());
    assert_eq!(restored.refusal(), None);
    assert_eq!(
        restored.pending_close_evidence(&child),
        vec![first[0].clone()]
    );
    let SettlementProof::Resource(slots) = &snapshot.settlements[0].proof else {
        panic!("resource debt");
    };
    assert_eq!(slots.iter().flatten().count(), 2);
    bounded(world.close(&child)).await.unwrap();
    let third = audit.observations(&child);
    assert_eq!(third.len(), 4);
    assert_eq!(third[3], first[0]);
    assert_eq!(resources.closes.load(Ordering::SeqCst), 2);
    assert_eq!(audit.completions(), 1);
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closed)
    );
}

#[tokio::test]
async fn joined_close_waiters_keep_failed_generation_after_successful_retry_and_caller_loss() {
    let (world, resources, audit, _, room) = settlement_world(vec![released()]);
    let root = bounded(world.root()).await;
    let child = bounded(
        world
            .coordinator
            .spawn(world.command(&root, "joined-close", "task")),
    )
    .await
    .unwrap()
    .child;
    let gate = audit.hold_released(&child);
    let mut caller = Box::pin(world.close(&child));
    poll_pending(caller.as_mut());
    bounded(gate.entered.notified()).await;
    drop(caller);
    let mut first = Box::pin(world.close(&child));
    let mut second = Box::pin(world.close(&child));
    poll_pending(first.as_mut());
    poll_pending(second.as_mut());
    assert!(room.try_reserve());
    room.release();
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    gate.release.notify_one();
    let failure = bounded(first).await;
    assert_eq!(failure, Err(OwnershipFailure::Audit(PortFailure::Rejected)));
    bounded(world.close(&child)).await.unwrap();
    assert_eq!(
        bounded(second).await,
        failure,
        "a later successful generation cannot rewrite an existing waiter"
    );
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    let observations = audit.observations(&child);
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0], observations[1]);
}

#[tokio::test]
async fn parent_joins_failed_independent_child_and_preserves_its_first_close_owner() {
    let (world, resources, audit, store, _) = settlement_world(vec![released()]);
    let root = bounded(world.root()).await;
    world.bind_root(&root);
    let child = bounded(
        world
            .coordinator
            .spawn(world.command(&root, "independent-close", "task")),
    )
    .await
    .unwrap()
    .child;
    let gate = audit.hold_released(&child);
    let mut child_close = Box::pin(close_with(
        &world.coordinator,
        &child,
        LifetimeCause::TerminalFailure,
        Initiator::Runtime,
    ));
    poll_pending(child_close.as_mut());
    bounded(gate.entered.notified()).await;
    let exact = audit.observations(&child)[0].clone();
    *store.watch_intent.lock().unwrap() = Some(root.clone());
    let mut parent_close = Box::pin(close_with(
        &world.coordinator,
        &root,
        LifetimeCause::Deletion,
        Initiator::Host(actor("delete-parent")),
    ));
    poll_pending(parent_close.as_mut());
    // On this current-thread runtime, the immediate MemoryStore write signals
    // without yielding; the parent worker then runs until the held child wait.
    bounded(store.intent_saved.notified()).await;
    gate.release.notify_one();
    assert_eq!(
        bounded(child_close).await,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    assert_eq!(
        bounded(parent_close).await,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    let snapshot = bounded(world.store.read()).await.unwrap();
    let restored = OwnershipGraph::restore(snapshot);
    assert_eq!(restored.refusal(), None);
    assert_eq!(
        restored.close_operation(&child),
        exact.close_operation.as_ref()
    );
    assert_eq!(
        restored.close_cause(&child),
        Some(&LifetimeCause::TerminalFailure)
    );
    assert_eq!(restored.close_initiator(&child), Some(&Initiator::Runtime));
    assert_eq!(
        world.coordinator.close_cause(&root),
        Some(LifetimeCause::Deletion)
    );
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    bounded(world.close(&root)).await.unwrap();
    let observations = audit.observations(&child);
    assert_eq!(observations, vec![exact.clone(), exact]);
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        world.coordinator.close_cause(&child),
        Some(LifetimeCause::TerminalFailure)
    );
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        *resources.causes.lock().unwrap(),
        vec![(LifetimeCause::TerminalFailure, Initiator::Runtime)]
    );
    assert_eq!(audit.completions(), 2);
}

#[tokio::test]
async fn rejected_safety_write_blocks_completion_and_final_rejection_retries_only_writer() {
    let (world, resources, audit, store, _) = settlement_world(vec![released()]);
    let root = bounded(world.root()).await;
    let child = bounded(
        world
            .coordinator
            .spawn(world.command(&root, "write-phases", "task")),
    )
    .await
    .unwrap()
    .child;
    store.reject_safety.store(true, Ordering::SeqCst);
    store.reject_final.store(true, Ordering::SeqCst);
    assert_eq!(
        bounded(world.close(&child)).await,
        Err(OwnershipFailure::Store(PortFailure::Rejected))
    );
    assert_eq!(store.safety_rejections.load(Ordering::SeqCst), 1);
    assert_eq!(audit.completions(), 0);
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closing)
    );
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        bounded(world.close(&child)).await,
        Err(OwnershipFailure::Store(PortFailure::Rejected))
    );
    assert_eq!(store.final_rejections.load(Ordering::SeqCst), 1);
    assert_eq!(audit.completions(), 1);
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closed)
    );
    let retained = bounded(world.store.read()).await.unwrap();
    assert_eq!(
        retained
            .lifetimes
            .iter()
            .find(|row| row.lifetime_id == child)
            .unwrap()
            .state,
        LifetimeState::Closing
    );
    let restored = OwnershipGraph::restore(retained);
    assert_eq!(restored.refusal(), None);
    assert!(restored.pending_close_evidence(&child).is_empty());
    let audits_before = audit.records.lock().unwrap().len();
    bounded(world.close(&child)).await.unwrap();
    assert_eq!(audit.records.lock().unwrap().len(), audits_before);
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        bounded(world.store.read())
            .await
            .unwrap()
            .lifetimes
            .iter()
            .find(|row| row.lifetime_id == child)
            .unwrap()
            .state,
        LifetimeState::Closed
    );
}
