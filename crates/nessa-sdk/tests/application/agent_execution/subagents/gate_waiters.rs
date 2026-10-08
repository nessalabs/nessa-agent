//! SDK-issued participation waits retain their generation across caller-waker faults.
use super::*;
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::domain::agent_execution::subagents::CloseEvidenceDetail;
use std::{
    future::Future,
    task::{Context, Wake, Waker},
};

struct HeldObservationAudit {
    target: Mutex<Option<AgentLifetimeId>>,
    entered: Notify,
    release: Notify,
    records: Mutex<Vec<OwnershipEvidence>>,
}
#[async_trait]
impl OwnershipAudit for HeldObservationAudit {
    async fn record(&self, record: &OwnershipEvidence) -> Result<(), PortFailure> {
        self.records.lock().unwrap().push(record.clone());
        let selected = record.child_lifetime.as_ref() == self.target.lock().unwrap().as_ref()
            && matches!(
                record.close_detail,
                Some(CloseEvidenceDetail::ResourceObservation {
                    physical: PhysicalFact::Released,
                    ..
                })
            );
        if selected {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}
struct FaultPayload(Arc<AtomicUsize>);
impl Drop for FaultPayload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("gate caller destructor fault payload");
    }
}
struct DestructorWake {
    wakes: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    owned_drops: Arc<AtomicUsize>,
    payload_drops: Arc<AtomicUsize>,
}
impl Wake for DestructorWake {
    fn wake(self: Arc<Self>) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }
}
impl Drop for DestructorWake {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        if tokio::task::try_id().is_some() {
            self.owned_drops.fetch_add(1, Ordering::SeqCst);
        }
        std::panic::panic_any(FaultPayload(self.payload_drops.clone()));
    }
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("gate waiter deadline")
}

#[test]
fn panicking_gate_join_waker_drop_is_contained_in_owned_publisher() {
    const CHILD: &str = "NESSA_GATE_JOIN_WAKER_DROP_CHILD";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let audit = Arc::new(HeldObservationAudit {
                    target: Mutex::new(None),
                    entered: Notify::new(),
                    release: Notify::new(),
                    records: Mutex::new(Vec::new()),
                });
                let store = Arc::new(MemoryOwnershipStore::new());
                let factory = ScriptFactory::new();
                let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
                    store: store.clone(),
                    audit: audit.clone(),
                    factory: factory.clone(),
                    room: Arc::new(LiveCapacity::new(1)),
                });
                let root = bounded(coordinator.open_root(
                    SessionId::new("gate-waker-root").unwrap(),
                    Initiator::Runtime,
                ))
                .await
                .unwrap();
                let resources = Arc::new(ScriptResources {
                    closes: AtomicUsize::new(0),
                    report: ResourceReport {
                        physical: PhysicalFact::Released,
                        evidence: EvidenceFact::Acknowledged,
                    },
                    hold: None,
                });
                coordinator
                    .bind_resources(root.clone(), resources.clone())
                    .unwrap();
                let gate = coordinator.participation(&root).unwrap();
                *audit.target.lock().unwrap() = Some(root.clone());
                bounded(
                    gate.seal_for_host(&ActionContext::new("owner", "desktop", "seal").unwrap()),
                )
                .await
                .unwrap();
                bounded(gate.note_attachment(true, true)).await;
                bounded(audit.entered.notified()).await;
                assert!(gate.is_sealed());
                assert_eq!(
                    resources.closes.load(Ordering::SeqCst),
                    0,
                    "external attachment report supplies cleanup"
                );
                let wakes = Arc::new(AtomicUsize::new(0));
                let drops = Arc::new(AtomicUsize::new(0));
                let owned_drops = Arc::new(AtomicUsize::new(0));
                let payload_drops = Arc::new(AtomicUsize::new(0));
                let caller = Waker::from(Arc::new(DestructorWake {
                    wakes: wakes.clone(),
                    drops: drops.clone(),
                    owned_drops: owned_drops.clone(),
                    payload_drops: payload_drops.clone(),
                }));
                let mut faulty = Box::pin(gate.join_descendants());
                assert!(faulty
                    .as_mut()
                    .poll(&mut Context::from_waker(&caller))
                    .is_pending());
                drop(caller);
                assert_eq!(
                    drops.load(Ordering::SeqCst),
                    0,
                    "pending SDK wait owns the last caller waker"
                );
                let healthy_coordinator = coordinator.clone();
                let healthy_root = root.clone();
                let (registered, registered_rx) = tokio::sync::oneshot::channel();
                let healthy = tokio::spawn(async move {
                    let mut waiting = Box::pin(healthy_coordinator.end_lifetime(CloseCommand {
                        lifetime: healthy_root,
                        cause: LifetimeCause::HostClose,
                        initiator: Initiator::Runtime,
                        external_attachment: true,
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
                bounded(registered_rx).await.unwrap();
                audit.release.notify_one();
                bounded(healthy).await.unwrap().unwrap();
                bounded(faulty).await.unwrap();
                assert!(wakes.load(Ordering::SeqCst) > 0);
                assert_eq!(drops.load(Ordering::SeqCst), 1);
                assert_eq!(
                    owned_drops.load(Ordering::SeqCst),
                    1,
                    "last caller waker drops in SDK publisher task"
                );
                assert_eq!(
                    payload_drops.load(Ordering::SeqCst),
                    0,
                    "fault payload stays inside SDK boundary before Tokio can drop it"
                );
                assert_eq!(resources.closes.load(Ordering::SeqCst), 0);
                assert_eq!(factory.prepares.load(Ordering::SeqCst), 0);
                {
                    let records = audit.records.lock().unwrap();
                    assert_eq!(
                        records
                            .iter()
                            .filter(|record| matches!(
                                record.close_detail,
                                Some(CloseEvidenceDetail::ResourceObservation {
                                    physical: PhysicalFact::Released,
                                    ..
                                })
                            ))
                            .count(),
                        1
                    );
                    assert_eq!(
                        records
                            .iter()
                            .filter(|record| record.close_detail
                                == Some(CloseEvidenceDetail::Completion))
                            .count(),
                        1
                    );
                }
                let snapshot = bounded(store.read()).await.unwrap();
                let row = snapshot
                    .lifetimes
                    .iter()
                    .find(|row| row.lifetime_id == root)
                    .unwrap();
                assert_eq!(row.state, LifetimeState::Closed);
            });
        return;
    }
    let log_path =
        std::env::temp_dir().join(format!("nessa-gate-join-drop-{}.log", std::process::id()));
    let log = std::fs::File::create(&log_path).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "application::agent_execution::subagents::gate_waiters::panicking_gate_join_waker_drop_is_contained_in_owned_publisher", "--nocapture"]).env(CHILD, "1").stdout(log.try_clone().unwrap()).stderr(log).spawn().unwrap();
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "gate waker child failed {status}: {}",
                std::fs::read_to_string(&log_path).unwrap()
            );
            break;
        }
        if started.elapsed() > Duration::from_secs(8) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("gate waker child timed out");
        }
        std::thread::yield_now();
    }
}
