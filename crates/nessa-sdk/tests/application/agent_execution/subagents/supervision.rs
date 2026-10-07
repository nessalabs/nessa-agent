//! Actual factory construction, poll and Ready-future destruction boundaries.
use super::*;
use nessa_sdk::application::agent_execution::subagents::LiveRoom;
use nessa_sdk::domain::agent_execution::subagents::{
    AbsenceProof, CloseEvidenceDetail, SettlementProof,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{atomic::AtomicBool, Weak},
    task::{Context, Poll, Waker},
};

type Preparation = Result<PreparedChild, PrepareFailure>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FactoryFault {
    Constructor,
    Poll,
    PollPayload,
    ReadyOwnerDrop,
    ReadyRejectedDrop,
}

struct FaultFactory {
    fault: FactoryFault,
    calls: AtomicUsize,
    child: Mutex<Option<AgentLifetimeId>>,
    gate: Mutex<Option<Arc<dyn OwnedLifetime>>>,
    entered: Arc<Notify>,
    release: Option<Arc<Notify>>,
    resources: Arc<ScriptResources>,
    submits: Arc<AtomicUsize>,
    room: Arc<LiveCapacity>,
    observed_before_drop: Arc<AtomicBool>,
    payload_drops: Arc<AtomicUsize>,
}

struct FaultingPayload(Arc<AtomicUsize>);
impl Drop for FaultingPayload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("factory panic payload destructor");
    }
}

struct PreparationFuture {
    fault: FactoryFault,
    output: Option<Preparation>,
    wait: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
    entered: Arc<Notify>,
    first_poll: bool,
    resources: Weak<ScriptResources>,
    room: Arc<LiveCapacity>,
    observed_before_drop: Arc<AtomicBool>,
    payload_drops: Arc<AtomicUsize>,
}

impl Future for PreparationFuture {
    type Output = Preparation;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.first_poll {
            self.first_poll = false;
            self.entered.notify_one();
        }
        if let Some(wait) = &mut self.wait {
            if wait.as_mut().poll(context).is_pending() {
                return Poll::Pending;
            }
        }
        match self.fault {
            FactoryFault::Poll => panic!("factory preparation poll"),
            FactoryFault::PollPayload => {
                std::panic::panic_any(FaultingPayload(self.payload_drops.clone()))
            }
            _ => Poll::Ready(self.output.take().expect("one actual Ready output")),
        }
    }
}

impl Drop for PreparationFuture {
    fn drop(&mut self) {
        match self.fault {
            FactoryFault::ReadyOwnerDrop => {
                // Factory, returned PreparedChild, temporary upgrade and coordinator
                // must all retain the actual owner before this future is destroyed.
                let resources = self.resources.upgrade().unwrap();
                self.observed_before_drop
                    .store(Arc::strong_count(&resources) >= 4, Ordering::SeqCst);
                panic!("Ready factory future destructor");
            }
            FactoryFault::ReadyRejectedDrop => {
                let free = self.room.try_reserve();
                self.observed_before_drop.store(free, Ordering::SeqCst);
                if free {
                    self.room.release();
                }
                panic!("Ready rejected factory future destructor");
            }
            _ => {}
        }
    }
}

// Deliberately desugared: a constructor panic occurs before any future exists;
// poll and destructor faults belong to the returned future, not an async body.
impl ChildFactory for FaultFactory {
    fn prepare<'life0, 'async_trait>(
        &'life0 self,
        request: PrepareRequest,
    ) -> Pin<Box<dyn Future<Output = Preparation> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.child.lock().unwrap() = Some(request.child);
        *self.gate.lock().unwrap() = Some(request.owned_lifetime);
        if self.fault == FactoryFault::Constructor {
            panic!("factory preparation constructor");
        }
        let output = if self.fault == FactoryFault::ReadyOwnerDrop {
            Some(Ok(PreparedChild {
                resources: self.resources.clone(),
                submit: Arc::new(ScriptSubmit {
                    submits: self.submits.clone(),
                    result: Ok(TaskReceiptId::new("unused-receipt").unwrap()),
                }),
            }))
        } else {
            Some(Err(PrepareFailure {
                failure: PortFailure::Rejected,
                cleanup: None,
            }))
        };
        let wait = self.release.as_ref().map(|release| {
            let release = release.clone();
            Box::pin(async move { release.notified().await })
                as Pin<Box<dyn Future<Output = ()> + Send>>
        });
        Box::pin(PreparationFuture {
            fault: self.fault,
            output,
            wait,
            entered: self.entered.clone(),
            first_poll: true,
            resources: Arc::downgrade(&self.resources),
            room: self.room.clone(),
            observed_before_drop: self.observed_before_drop.clone(),
            payload_drops: self.payload_drops.clone(),
        })
    }
}

fn fault_world(fault: FactoryFault, held: bool) -> (World, Arc<FaultFactory>, Arc<LiveCapacity>) {
    let mut world = World::new(1);
    let room = Arc::new(LiveCapacity::new(1));
    let factory = Arc::new(FaultFactory {
        fault,
        calls: AtomicUsize::new(0),
        child: Mutex::new(None),
        gate: Mutex::new(None),
        entered: Arc::new(Notify::new()),
        release: held.then(|| Arc::new(Notify::new())),
        resources: Arc::new(ScriptResources {
            closes: AtomicUsize::new(0),
            report: released(),
            hold: None,
        }),
        submits: Arc::new(AtomicUsize::new(0)),
        room: room.clone(),
        observed_before_drop: Arc::new(AtomicBool::new(false)),
        payload_drops: Arc::new(AtomicUsize::new(0)),
    });
    world.coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: world.store.clone(),
        audit: world.audit.clone(),
        factory: factory.clone(),
        room: room.clone(),
    });
    (world, factory, room)
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("owned factory journey must terminate")
}

fn poll_pending(future: Pin<&mut impl Future>) {
    assert!(future
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
}

async fn unknown_factory_fault(fault: FactoryFault) {
    let (world, factory, room) = fault_world(fault, false);
    let root = bounded(world.root()).await;
    world.bind_root(&root);
    let command = world.command(&root, "fault", "task");
    let result = bounded(world.coordinator.spawn(command.clone())).await;
    assert_eq!(
        result,
        Err(OwnershipFailure::Startup(PortFailure::Uncertain))
    );
    assert_eq!(bounded(world.coordinator.spawn(command)).await, result);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    assert_eq!(factory.submits.load(Ordering::SeqCst), 0);
    assert!(factory.gate.lock().unwrap().as_ref().unwrap().is_sealed());
    assert!(
        !room.try_reserve(),
        "unobserved factory absence cannot return capacity"
    );
    let child = factory.child.lock().unwrap().clone().unwrap();
    let snapshot = bounded(world.store.read()).await.unwrap();
    let restored = OwnershipGraph::restore(snapshot);
    assert_eq!(restored.refusal(), None);
    assert!(!restored.has_absence(&child));
    assert_eq!(factory.resources.closes.load(Ordering::SeqCst), 0);
    assert_eq!(
        bounded(world.close(&root)).await,
        Err(OwnershipFailure::Incomplete)
    );
    assert_eq!(factory.payload_drops.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn factory_constructor_panic_is_cached_uncertainty_without_absence() {
    unknown_factory_fault(FactoryFault::Constructor).await;
}

#[tokio::test]
async fn factory_poll_panic_is_cached_uncertainty_without_absence() {
    unknown_factory_fault(FactoryFault::Poll).await;
}

#[test]
fn factory_poll_payload_destructor_is_not_run() {
    const CHILD: &str = "NESSA_OWNED_FACTORY_PAYLOAD_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(unknown_factory_fault(FactoryFault::PollPayload));
        return;
    }
    let log_path =
        std::env::temp_dir().join(format!("nessa-factory-payload-{}.log", std::process::id()));
    let log = std::fs::File::create(&log_path).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "application::agent_execution::subagents::supervision::factory_poll_payload_destructor_is_not_run"])
        .env(CHILD, "1")
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn().unwrap();
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "factory payload subprocess failed: {status}: {}",
                std::fs::read_to_string(&log_path).unwrap()
            );
            break;
        }
        if started.elapsed() > Duration::from_secs(8) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("factory payload subprocess watchdog expired");
        }
        std::thread::yield_now();
    }
}

#[tokio::test]
async fn ready_prepared_owner_is_retained_before_future_drop_fault() {
    let (world, factory, room) = fault_world(FactoryFault::ReadyOwnerDrop, false);
    let root = bounded(world.root()).await;
    world.bind_root(&root);
    let command = world.command(&root, "owner-drop", "task");
    let first = bounded(world.coordinator.spawn(command.clone())).await;
    assert_eq!(
        first,
        Err(OwnershipFailure::Startup(PortFailure::Uncertain))
    );
    assert!(factory.observed_before_drop.load(Ordering::SeqCst));
    assert_eq!(factory.submits.load(Ordering::SeqCst), 0);
    assert!(!room.try_reserve());
    let child = factory.child.lock().unwrap().clone().unwrap();
    let restored = OwnershipGraph::restore(bounded(world.store.read()).await.unwrap());
    assert_eq!(restored.refusal(), None);
    assert!(!restored.has_absence(&child));
    assert_eq!(
        restored
            .spawn_progress(&command.request_id)
            .unwrap()
            .known(),
        nessa_sdk::domain::agent_execution::subagents::KnownMilestone::Prepared
    );
    bounded(world.close(&root)).await.unwrap();
    assert_eq!(factory.resources.closes.load(Ordering::SeqCst), 1);
    assert!(room.try_reserve());
    room.release();
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        bounded(world.coordinator.spawn(command)).await,
        first,
        "later Closed history must not replace the admitted terminal failure"
    );
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    assert_eq!(factory.resources.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ready_rejected_absence_returns_capacity_before_future_drop_fault() {
    let (world, factory, room) = fault_world(FactoryFault::ReadyRejectedDrop, false);
    let root = bounded(world.root()).await;
    world.bind_root(&root);
    let command = world.command(&root, "rejected-drop", "task");
    let first = bounded(world.coordinator.spawn(command.clone())).await;
    assert_eq!(
        first,
        Err(OwnershipFailure::Startup(PortFailure::Uncertain))
    );
    assert!(factory.observed_before_drop.load(Ordering::SeqCst));
    assert!(room.try_reserve());
    room.release();
    assert_eq!(factory.resources.closes.load(Ordering::SeqCst), 0);
    let child = factory.child.lock().unwrap().clone().unwrap();
    let snapshot = bounded(world.store.read()).await.unwrap();
    let absence = snapshot
        .settlements
        .iter()
        .find(|row| row.target == child)
        .unwrap();
    let SettlementProof::Absence(proof) = &absence.proof else {
        panic!("Ready Rejected+None must retain actual absence");
    };
    assert_eq!(
        proof.proof(),
        &AbsenceProof::PreparationRejectedWithoutOwner(command.request_id.clone())
    );
    assert_eq!(
        proof.record().close_detail,
        Some(CloseEvidenceDetail::Absence(proof.proof().clone()))
    );
    assert!(OwnershipGraph::restore(snapshot).refusal().is_none());
    bounded(world.close(&root)).await.unwrap();
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(bounded(world.coordinator.spawn(command)).await, first);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    assert_eq!(factory.submits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dropped_spawn_caller_and_two_joiners_receive_one_cached_factory_failure() {
    let (world, factory, _) = fault_world(FactoryFault::Poll, true);
    let root = bounded(world.root()).await;
    world.bind_root(&root);
    let command = world.command(&root, "joined-fault", "task");
    let mut caller = Box::pin(world.coordinator.spawn(command.clone()));
    poll_pending(caller.as_mut());
    bounded(factory.entered.notified()).await;
    drop(caller);
    let mut first = Box::pin(world.coordinator.spawn(command.clone()));
    let mut second = Box::pin(world.coordinator.spawn(command.clone()));
    poll_pending(first.as_mut());
    poll_pending(second.as_mut());
    factory.release.as_ref().unwrap().notify_one();
    let first = bounded(first).await;
    let second = bounded(second).await;
    assert_eq!(
        first,
        Err(OwnershipFailure::Startup(PortFailure::Uncertain))
    );
    assert_eq!(second, first);
    assert_eq!(
        bounded(world.coordinator.spawn(command.clone())).await,
        first
    );
    let mut conflicting = command;
    conflicting.task = "different task".into();
    assert_eq!(
        bounded(world.coordinator.spawn(conflicting)).await,
        Err(OwnershipFailure::Domain(OwnershipError::RequestConflict))
    );
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    assert_eq!(factory.submits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn no_room_request_can_admit_after_another_child_releases_capacity() {
    let world = World::new(1);
    let root = bounded(world.root()).await;
    world.bind_root(&root);
    let occupied = bounded(
        world
            .coordinator
            .spawn(world.command(&root, "occupied", "task")),
    )
    .await
    .unwrap();
    let retry = world.command(&root, "no-room", "other task");
    assert_eq!(
        bounded(world.coordinator.spawn(retry.clone())).await,
        Err(OwnershipFailure::Domain(OwnershipError::NoRoom))
    );
    assert_eq!(world.factory.prepares(), 1);
    assert!(bounded(world.store.read())
        .await
        .unwrap()
        .spawns
        .iter()
        .all(|row| row.binding.request_id != retry.request_id));
    bounded(world.close(&occupied.child)).await.unwrap();
    let admitted = bounded(world.coordinator.spawn(retry.clone()))
        .await
        .unwrap();
    assert_ne!(admitted.child, occupied.child);
    assert_eq!(world.factory.prepares(), 2);
    assert_eq!(world.factory.submits(), 2);
    assert_eq!(
        bounded(world.coordinator.spawn(retry)).await.unwrap(),
        admitted
    );
    assert_eq!(world.factory.prepares(), 2);
}
