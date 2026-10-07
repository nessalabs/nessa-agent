//! Actual injected audit/store effect boundaries and publication-waker faults.
use super::*;
use nessa_sdk::domain::agent_execution::subagents::CloseEvidenceDetail;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Wake, Waker},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    Constructor,
    Poll,
    ReadyDrop,
}
struct PortFuture<'a> {
    inner: Pin<Box<dyn Future<Output = Result<(), PortFailure>> + Send + 'a>>,
    fault: Option<Fault>,
    ready: bool,
}
impl Future for PortFuture<'_> {
    type Output = Result<(), PortFailure>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.fault == Some(Fault::Poll) {
            panic!("injected port poll");
        }
        let result = self.inner.as_mut().poll(cx);
        self.ready |= result.is_ready();
        result
    }
}
impl Drop for PortFuture<'_> {
    fn drop(&mut self) {
        if self.ready && self.fault == Some(Fault::ReadyDrop) {
            panic!("injected Ready port future Drop");
        }
    }
}
struct FaultAudit {
    fault: Mutex<Option<Fault>>,
    selected: Option<CloseEvidenceDetail>,
    observations: AtomicUsize,
    completions: AtomicUsize,
    reject_reservation: Mutex<bool>,
    reject_ready: bool,
}
impl OwnershipAudit for FaultAudit {
    fn record<'s, 'e, 'f>(
        &'s self,
        evidence: &'e OwnershipEvidence,
    ) -> Pin<Box<dyn Future<Output = Result<(), PortFailure>> + Send + 'f>>
    where
        's: 'f,
        'e: 'f,
        Self: 'f,
    {
        if evidence.close_detail == Some(CloseEvidenceDetail::Completion) {
            self.completions.fetch_add(1, Ordering::SeqCst);
        }
        let selected = match &self.selected {
            Some(detail) => evidence.close_detail.as_ref() == Some(detail),
            None => evidence.after == OwnershipMeaning::Closing && evidence.close_detail.is_none(),
        };
        let fault = if selected {
            self.observations.fetch_add(1, Ordering::SeqCst);
            self.fault.lock().unwrap().take()
        } else {
            None
        };
        if fault == Some(Fault::Constructor) {
            panic!("audit constructor");
        }
        let reject = (evidence.after == OwnershipMeaning::Reserved
            && std::mem::take(&mut *self.reject_reservation.lock().unwrap()))
            || (self.reject_ready && selected && fault == Some(Fault::ReadyDrop));
        Box::pin(PortFuture {
            inner: Box::pin(async move {
                if reject {
                    Err(PortFailure::Rejected)
                } else {
                    Ok(())
                }
            }),
            fault,
            ready: false,
        })
    }
}
fn fault_audit(fault: Fault, selected: CloseEvidenceDetail) -> Arc<FaultAudit> {
    Arc::new(FaultAudit {
        fault: Mutex::new(Some(fault)),
        selected: Some(selected),
        observations: AtomicUsize::new(0),
        completions: AtomicUsize::new(0),
        reject_reservation: Mutex::new(false),
        reject_ready: false,
    })
}
fn replace_ports(
    world: &mut World,
    audit: Arc<dyn OwnershipAudit>,
    store: Arc<dyn OwnershipStore>,
) {
    world.coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        audit,
        store,
        factory: world.factory.clone(),
        room: Arc::new(LiveCapacity::new(2)),
    });
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("owned effect must terminalize")
}
#[tokio::test]
async fn audit_constructor_poll_and_ready_drop_preserve_exact_observation_authority() {
    for fault in [Fault::Constructor, Fault::Poll, Fault::ReadyDrop] {
        let mut world = World::new(2);
        let audit = fault_audit(
            fault,
            CloseEvidenceDetail::ResourceObservation {
                physical: PhysicalFact::Released,
                provider_evidence: EvidenceFact::Acknowledged,
            },
        );
        let store = world.store.clone();
        replace_ports(&mut world, audit.clone(), store);
        let root = world.root().await;
        let child = world
            .coordinator
            .spawn(world.command(&root, "audit-effect", "task"))
            .await
            .unwrap()
            .child;
        assert_eq!(
            bounded(world.close(&child)).await,
            Err(OwnershipFailure::Audit(PortFailure::Uncertain)),
            "{fault:?}"
        );
        assert_eq!(
            world.coordinator.lifetime_state(&child),
            Some(LifetimeState::Closing)
        );
        assert_eq!(
            world.factory.children.lock().unwrap()[0]
                .closes
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(audit.completions.load(Ordering::SeqCst), 0);
        let snapshot = world.store.read().await.unwrap();
        let proof = match &snapshot
            .settlements
            .iter()
            .find(|row| row.target == child)
            .unwrap()
            .proof
        {
            nessa_sdk::domain::agent_execution::subagents::SettlementProof::Resource(slots) => {
                slots[2].as_ref().unwrap()
            }
            _ => panic!("actual resource proof"),
        };
        assert_eq!(
            proof.acknowledgement(),
            if fault == Fault::ReadyDrop {
                EvidenceFact::Acknowledged
            } else {
                EvidenceFact::Pending
            }
        );
        bounded(world.close(&child)).await.unwrap();
        assert_eq!(
            audit.observations.load(Ordering::SeqCst),
            if fault == Fault::ReadyDrop { 1 } else { 2 }
        );
        assert_eq!(audit.completions.load(Ordering::SeqCst), 1);
        assert_eq!(
            world.factory.children.lock().unwrap()[0]
                .closes
                .load(Ordering::SeqCst),
            1
        );
    }
}

struct FaultStore {
    memory: Arc<MemoryOwnershipStore>,
    fault: Mutex<Option<Fault>>,
    selected: Mutex<Option<AgentLifetimeId>>,
    terminal_writes: AtomicUsize,
    reject_ready: bool,
}
impl OwnershipStore for FaultStore {
    fn write<'s, 'v, 'f>(
        &'s self,
        snapshot: &'v OwnershipSnapshot,
    ) -> Pin<Box<dyn Future<Output = Result<(), PortFailure>> + Send + 'f>>
    where
        's: 'f,
        'v: 'f,
        Self: 'f,
    {
        let terminal = self.selected.lock().unwrap().as_ref().is_some_and(|id| {
            snapshot
                .lifetimes
                .iter()
                .any(|row| &row.lifetime_id == id && row.state == LifetimeState::Closed)
        });
        let fault = if terminal {
            self.terminal_writes.fetch_add(1, Ordering::SeqCst);
            self.fault.lock().unwrap().take()
        } else {
            None
        };
        if fault == Some(Fault::Constructor) {
            panic!("store constructor");
        }
        Box::pin(PortFuture {
            inner: Box::pin(async move {
                if self.reject_ready && fault == Some(Fault::ReadyDrop) {
                    Err(PortFailure::Rejected)
                } else {
                    self.memory.write(snapshot).await
                }
            }),
            fault,
            ready: false,
        })
    }
    fn read<'s, 'f>(
        &'s self,
    ) -> Pin<Box<dyn Future<Output = Result<OwnershipSnapshot, PortFailure>> + Send + 'f>>
    where
        's: 'f,
        Self: 'f,
    {
        Box::pin(self.memory.read())
    }
}
#[tokio::test]
async fn store_constructor_poll_and_ready_drop_keep_completion_and_writer_only_retry() {
    for fault in [Fault::Constructor, Fault::Poll, Fault::ReadyDrop] {
        let mut world = World::new(2);
        let store = Arc::new(FaultStore {
            memory: world.store.clone(),
            fault: Mutex::new(Some(fault)),
            selected: Mutex::new(None),
            terminal_writes: AtomicUsize::new(0),
            reject_ready: false,
        });
        let audit = world.audit.clone();
        replace_ports(&mut world, audit, store.clone());
        let root = world.root().await;
        let child = world
            .coordinator
            .spawn(world.command(&root, "store-effect", "task"))
            .await
            .unwrap()
            .child;
        *store.selected.lock().unwrap() = Some(child.clone());
        assert_eq!(
            bounded(world.close(&child)).await,
            Err(OwnershipFailure::Store(PortFailure::Uncertain)),
            "{fault:?}"
        );
        assert_eq!(
            world.coordinator.lifetime_state(&child),
            Some(LifetimeState::Closed)
        );
        let snapshot = world.store.read().await.unwrap();
        assert_eq!(
            snapshot
                .lifetimes
                .iter()
                .find(|row| row.lifetime_id == child)
                .unwrap()
                .state,
            if fault == Fault::ReadyDrop {
                LifetimeState::Closed
            } else {
                LifetimeState::Closing
            }
        );
        let audits = world.audit.records.lock().unwrap().len();
        bounded(world.close(&child)).await.unwrap();
        assert_eq!(world.audit.records.lock().unwrap().len(), audits);
        assert_eq!(
            world.factory.children.lock().unwrap()[0]
                .closes
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(store.terminal_writes.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn recovery_audit_fault_does_not_strand_original_publication_failure() {
    let mut world = World::new(2);
    // Constructor failure occurs inside THIS rejected admission's fallback
    // revocation publication, rather than in the primary audit call.
    let audit = Arc::new(FaultAudit {
        fault: Mutex::new(Some(Fault::Constructor)),
        selected: None,
        observations: AtomicUsize::new(0),
        completions: AtomicUsize::new(0),
        reject_reservation: Mutex::new(true),
        reject_ready: false,
    });
    let store = world.store.clone();
    replace_ports(&mut world, audit.clone(), store);
    let root = world.root().await;
    let command = world.command(&root, "recover-fault", "task");
    let first = bounded(world.coordinator.spawn(command.clone())).await;
    assert_eq!(first, Err(OwnershipFailure::Audit(PortFailure::Rejected)));
    let child = world.factory.last_child.lock().unwrap().clone();
    assert!(child.is_none());
    assert_eq!(audit.observations.load(Ordering::SeqCst), 1);
    assert!(
        world.store.read().await.unwrap().spawns.is_empty(),
        "private rejection cannot acquire durable identity permission"
    );
    assert_eq!(bounded(world.coordinator.spawn(command)).await, first);
    bounded(world.close(&root)).await.unwrap();
    assert_eq!(world.factory.prepares(), 0);
}

struct BadPayload;
impl Drop for BadPayload {
    fn drop(&mut self) {
        panic!("caller wake payload Drop");
    }
}
struct BadWake {
    calls: AtomicUsize,
}
impl Wake for BadWake {
    fn wake(self: Arc<Self>) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::panic::panic_any(BadPayload);
    }
}
#[test]
fn panicking_caller_waker_and_payload_do_not_strand_other_close_waiters() {
    const CHILD: &str = "NESSA_SETTLEMENT_WAKER_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let world = World::new(1);
                let root = world.root().await;
                let release = Arc::new(Notify::new());
                *world.factory.hold_next.lock().unwrap() = Some(release.clone());
                let child = world
                    .coordinator
                    .spawn(world.command(&root, "wake", "task"))
                    .await
                    .unwrap()
                    .child;
                let waker = Arc::new(BadWake {
                    calls: AtomicUsize::new(0),
                });
                let mut faulty = Box::pin(world.close(&child));
                assert!(faulty
                    .as_mut()
                    .poll(&mut Context::from_waker(&Waker::from(waker.clone())))
                    .is_pending());
                let healthy_coordinator = world.coordinator.clone();
                let healthy_child = child.clone();
                let (registered, registered_rx) = tokio::sync::oneshot::channel();
                let healthy = tokio::spawn(async move {
                    let mut waiting = Box::pin(healthy_coordinator.end_lifetime(CloseCommand {
                        lifetime: healthy_child,
                        cause: LifetimeCause::HostClose,
                        initiator: Initiator::Runtime,
                        external_attachment: false,
                        timeout: None,
                    }));
                    let mut registered = Some(registered);
                    std::future::poll_fn(|cx| {
                        let result = waiting.as_mut().poll(cx);
                        if let Some(registered) = registered.take() {
                            registered.send(()).unwrap();
                        }
                        result
                    })
                    .await
                });
                registered_rx.await.unwrap();
                release.notify_one();
                for _ in 0..1000 {
                    if healthy.is_finished() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                assert!(
                    healthy.is_finished(),
                    "registered healthy waiter needs its terminal wake"
                );
                healthy.await.unwrap().unwrap();
                bounded(faulty).await.unwrap();
                assert!(waker.calls.load(Ordering::SeqCst) > 0);
                assert_eq!(
                    world.factory.children.lock().unwrap()[0]
                        .closes
                        .load(Ordering::SeqCst),
                    1
                );
            });
        return;
    }
    let log_path =
        std::env::temp_dir().join(format!("nessa-settlement-waker-{}.log", std::process::id()));
    let log = std::fs::File::create(&log_path).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "application::agent_execution::subagents::effects::panicking_caller_waker_and_payload_do_not_strand_other_close_waiters", "--nocapture"]).env(CHILD, "1").stdout(log.try_clone().unwrap()).stderr(log).spawn().unwrap();
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "waker child failed {status}: {}",
                std::fs::read_to_string(&log_path).unwrap()
            );
            break;
        }
        if started.elapsed() > Duration::from_secs(8) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("waker child timed out");
        }
        std::thread::yield_now();
    }
}

#[tokio::test]
async fn ready_audit_rejection_survives_future_drop() {
    let mut world = World::new(2);
    let mut audit = fault_audit(
        Fault::ReadyDrop,
        CloseEvidenceDetail::ResourceObservation {
            physical: PhysicalFact::Released,
            provider_evidence: EvidenceFact::Acknowledged,
        },
    );
    Arc::get_mut(&mut audit).unwrap().reject_ready = true;
    let memory = world.store.clone();
    replace_ports(&mut world, audit.clone(), memory);
    let root = world.root().await;
    let child = world
        .coordinator
        .spawn(world.command(&root, "audit-ready-rejected", "task"))
        .await
        .unwrap()
        .child;
    assert_eq!(
        bounded(world.close(&child)).await,
        Err(OwnershipFailure::Audit(PortFailure::Rejected))
    );
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closing)
    );
    bounded(world.close(&child)).await.unwrap();
    assert_eq!(audit.observations.load(Ordering::SeqCst), 2);
    assert_eq!(
        world.factory.children.lock().unwrap()[0]
            .closes
            .load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn ready_store_rejection_survives_future_drop() {
    let mut world = World::new(2);
    let store = Arc::new(FaultStore {
        memory: world.store.clone(),
        fault: Mutex::new(Some(Fault::ReadyDrop)),
        selected: Mutex::new(None),
        terminal_writes: AtomicUsize::new(0),
        reject_ready: true,
    });
    let audit = world.audit.clone();
    replace_ports(&mut world, audit, store.clone());
    let root = world.root().await;
    let child = world
        .coordinator
        .spawn(world.command(&root, "store-ready-rejected", "task"))
        .await
        .unwrap()
        .child;
    *store.selected.lock().unwrap() = Some(child.clone());
    assert_eq!(
        bounded(world.close(&child)).await,
        Err(OwnershipFailure::Store(PortFailure::Rejected))
    );
    assert_eq!(
        world.coordinator.lifetime_state(&child),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        world
            .store
            .read()
            .await
            .unwrap()
            .lifetimes
            .iter()
            .find(|row| row.lifetime_id == child)
            .unwrap()
            .state,
        LifetimeState::Closing
    );
    let audits = world.audit.records.lock().unwrap().len();
    bounded(world.close(&child)).await.unwrap();
    assert_eq!(world.audit.records.lock().unwrap().len(), audits);
    assert_eq!(store.terminal_writes.load(Ordering::SeqCst), 2);
    assert_eq!(
        world.factory.children.lock().unwrap()[0]
            .closes
            .load(Ordering::SeqCst),
        1
    );
}

struct BadDropPayload {
    drops: Arc<AtomicUsize>,
}
impl Drop for BadDropPayload {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        panic!("caller waker Drop payload destructor");
    }
}
struct BadDropWake {
    drops: Arc<AtomicUsize>,
    owned_drops: Arc<AtomicUsize>,
    payload_drops: Arc<AtomicUsize>,
}
impl Wake for BadDropWake {
    fn wake(self: Arc<Self>) {}
}
impl Drop for BadDropWake {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        if tokio::task::try_id().is_some() {
            self.owned_drops.fetch_add(1, Ordering::SeqCst);
        }
        std::panic::panic_any(BadDropPayload {
            drops: self.payload_drops.clone(),
        });
    }
}
#[test]
fn panicking_caller_waker_drop_does_not_strand_registered_close_waiters() {
    const CHILD: &str = "NESSA_SETTLEMENT_WAKER_DROP_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let world = World::new(1);
                let root = world.root().await;
                let release = Arc::new(Notify::new());
                *world.factory.hold_next.lock().unwrap() = Some(release.clone());
                let child = world
                    .coordinator
                    .spawn(world.command(&root, "wake-drop", "task"))
                    .await
                    .unwrap()
                    .child;
                let drops = Arc::new(AtomicUsize::new(0));
                let owned_drops = Arc::new(AtomicUsize::new(0));
                let payload_drops = Arc::new(AtomicUsize::new(0));
                let caller = Waker::from(Arc::new(BadDropWake {
                    drops: drops.clone(),
                    owned_drops: owned_drops.clone(),
                    payload_drops: payload_drops.clone(),
                }));
                let mut faulty = Box::pin(world.close(&child));
                assert!(faulty
                    .as_mut()
                    .poll(&mut Context::from_waker(&caller))
                    .is_pending());
                drop(caller);
                assert_eq!(
                    drops.load(Ordering::SeqCst),
                    0,
                    "registered SDK waker holds the caller"
                );
                let healthy_coordinator = world.coordinator.clone();
                let healthy_child = child.clone();
                let (registered, registered_rx) = tokio::sync::oneshot::channel();
                let healthy = tokio::spawn(async move {
                    let mut waiting = Box::pin(healthy_coordinator.end_lifetime(CloseCommand {
                        lifetime: healthy_child,
                        cause: LifetimeCause::HostClose,
                        initiator: Initiator::Runtime,
                        external_attachment: false,
                        timeout: None,
                    }));
                    let mut registered = Some(registered);
                    std::future::poll_fn(|cx| {
                        let result = waiting.as_mut().poll(cx);
                        if let Some(registered) = registered.take() {
                            registered.send(()).unwrap();
                        }
                        result
                    })
                    .await
                });
                registered_rx.await.unwrap();
                release.notify_one();
                for _ in 0..1000 {
                    if healthy.is_finished() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                assert!(
                    healthy.is_finished(),
                    "caller waker Drop must not suppress another registered wake"
                );
                healthy.await.unwrap().unwrap();
                bounded(faulty).await.unwrap();
                assert_eq!(drops.load(Ordering::SeqCst), 1);
                assert_eq!(
                    owned_drops.load(Ordering::SeqCst),
                    1,
                    "destructor ran in SDK publisher task"
                );
                assert_eq!(
                    payload_drops.load(Ordering::SeqCst),
                    0,
                    "SDK contains the fault payload before Tokio owns it"
                );
                assert_eq!(
                    world.factory.children.lock().unwrap()[0]
                        .closes
                        .load(Ordering::SeqCst),
                    1
                );
            });
        return;
    }
    let log_path = std::env::temp_dir().join(format!(
        "nessa-settlement-waker-drop-{}.log",
        std::process::id()
    ));
    let log = std::fs::File::create(&log_path).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "application::agent_execution::subagents::effects::panicking_caller_waker_drop_does_not_strand_registered_close_waiters", "--nocapture"]).env(CHILD,"1").stdout(log.try_clone().unwrap()).stderr(log).spawn().unwrap();
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "waker Drop child failed {status}: {}",
                std::fs::read_to_string(&log_path).unwrap()
            );
            break;
        }
        if started.elapsed() > Duration::from_secs(8) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("waker Drop child timed out");
        }
        std::thread::yield_now();
    }
}
