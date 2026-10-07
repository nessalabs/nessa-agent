//! Ready transfer retention across submit and resource future destruction.
use super::*;
use nessa_sdk::domain::agent_execution::subagents::KnownMilestone;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Wake, Waker},
};
use tokio::sync::oneshot;
use tokio::time::timeout;

const BOUND: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug)]
enum Fault {
    Clean,
    Constructor,
    Poll,
    ReadyDrop,
}
struct PayloadDrop;
impl Drop for PayloadDrop {
    fn drop(&mut self) {
        panic!("fixture payload destructor");
    }
}
fn fault(payload: bool) -> ! {
    if payload {
        std::panic::panic_any(PayloadDrop);
    }
    panic!("fixture effect fault");
}

// Deliberately a concrete manually returned future. An async method with a local
// Drop bomb typically destroys that local BEFORE returning Poll::Ready and cannot
// test the Ready -> install -> effect-future destruction ordering.
struct OutputFuture<T> {
    output: Option<T>,
    gate: Option<oneshot::Receiver<()>>,
    entered: Arc<Notify>,
    entered_once: bool,
    fault: Fault,
    drops: Arc<AtomicUsize>,
}
impl<T> Unpin for OutputFuture<T> {}
impl<T> Future for OutputFuture<T> {
    type Output = T;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        if !self.entered_once {
            self.entered_once = true;
            self.entered.notify_one();
        }
        if let Some(gate) = self.gate.as_mut() {
            if Pin::new(gate).poll(cx).is_pending() {
                return Poll::Pending;
            }
            self.gate = None;
        }
        if matches!(self.fault, Fault::Poll) {
            fault(false);
        }
        Poll::Ready(self.output.take().expect("future polled after Ready"))
    }
}
impl<T> Drop for OutputFuture<T> {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        if let Fault::ReadyDrop = self.fault {
            fault(false);
        }
    }
}

struct ReceiptSubmit {
    calls: AtomicUsize,
    drops: Arc<AtomicUsize>,
    fault: Fault,
    rejected: bool,
}
// These signatures match async_trait's desugaring, while allowing constructor
// faults BEFORE any Future exists. Do not add #[async_trait] to this impl.
impl InitialSubmit for ReceiptSubmit {
    fn submit<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        _task: &'life1 str,
        _request: &'life2 SpawnRequestId,
    ) -> Pin<Box<dyn Future<Output = Result<TaskReceiptId, PortFailure>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if matches!(self.fault, Fault::Constructor) {
            fault(false);
        }
        Box::pin(OutputFuture {
            output: Some(if self.rejected {
                Err(PortFailure::Rejected)
            } else {
                Ok(TaskReceiptId::new("ready-drop-receipt").unwrap())
            }),
            gate: None,
            entered: Arc::new(Notify::new()),
            entered_once: false,
            fault: self.fault,
            drops: self.drops.clone(),
        })
    }
}
#[derive(Clone, Copy, Debug)]
enum FactoryOutput {
    Prepared,
    RejectedNone,
    RejectedCleanup,
}
struct OutputFactory {
    calls: AtomicUsize,
    drops: Arc<AtomicUsize>,
    fault: Fault,
    output: FactoryOutput,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
    entered: Arc<Notify>,
    child: Mutex<Option<AgentLifetimeId>>,
    resources: Arc<ScriptResources>,
    submit: Arc<ReceiptSubmit>,
}
impl ChildFactory for OutputFactory {
    fn prepare<'life0, 'async_trait>(
        &'life0 self,
        request: PrepareRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PreparedChild, PrepareFailure>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.child.lock().unwrap() = Some(request.child);
        if matches!(self.fault, Fault::Constructor) {
            fault(false);
        }
        let output = match self.output {
            FactoryOutput::Prepared => Ok(PreparedChild {
                resources: self.resources.clone(),
                submit: self.submit.clone(),
            }),
            FactoryOutput::RejectedNone => Err(PrepareFailure {
                failure: PortFailure::Rejected,
                cleanup: None,
            }),
            FactoryOutput::RejectedCleanup => Err(PrepareFailure {
                failure: PortFailure::Rejected,
                cleanup: Some(self.resources.clone()),
            }),
        };
        Box::pin(OutputFuture {
            output: Some(output),
            gate: self.gate.lock().unwrap().take(),
            entered: self.entered.clone(),
            entered_once: false,
            fault: self.fault,
            drops: self.drops.clone(),
        })
    }
}
fn with_factory(factory: Arc<dyn ChildFactory>) -> World {
    let audit = ScriptAudit::new();
    let store = Arc::new(MemoryOwnershipStore::new());
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: store.clone(),
        audit: audit.clone(),
        factory,
        room: Arc::new(LiveCapacity::new(1)),
    });
    World {
        coordinator,
        factory: ScriptFactory::new(),
        audit,
        store,
    }
}
fn output_factory(
    output: FactoryOutput,
    fault: Fault,
    submit_fault: Fault,
) -> (Arc<OutputFactory>, oneshot::Sender<()>) {
    let (release, gate) = oneshot::channel();
    (
        Arc::new(OutputFactory {
            calls: AtomicUsize::new(0),
            drops: Arc::new(AtomicUsize::new(0)),
            fault,
            output,
            gate: Mutex::new(Some(gate)),
            entered: Arc::new(Notify::new()),
            child: Mutex::new(None),
            resources: Arc::new(ScriptResources {
                closes: AtomicUsize::new(0),
                report: released(),
                hold: None,
            }),
            submit: Arc::new(ReceiptSubmit {
                calls: AtomicUsize::new(0),
                drops: Arc::new(AtomicUsize::new(0)),
                fault: submit_fault,
                rejected: false,
            }),
        }),
        release,
    )
}

#[tokio::test]
async fn ready_factory_outputs_survive_effect_future_drop() {
    for output in [
        FactoryOutput::Prepared,
        FactoryOutput::RejectedCleanup,
        FactoryOutput::RejectedNone,
    ] {
        let (factory, release) = output_factory(output, Fault::ReadyDrop, Fault::Clean);
        let world = with_factory(factory.clone());
        let root = world.root().await;
        world.bind_root(&root);
        let command = world.command(&root, "ready-factory", "task");
        let coordinator = world.coordinator.clone();
        let admitted = command.clone();
        let first = tokio::spawn(async move { coordinator.spawn(admitted).await });
        timeout(BOUND, factory.entered.notified()).await.unwrap();
        release.send(()).unwrap();
        let expected = Err(OwnershipFailure::Startup(
            if matches!(output, FactoryOutput::Prepared) {
                PortFailure::Uncertain
            } else {
                PortFailure::Rejected
            },
        ));
        assert_eq!(timeout(BOUND, first).await.unwrap().unwrap(), expected);
        assert_eq!(factory.drops.load(Ordering::SeqCst), 1);
        assert_eq!(factory.submit.calls.load(Ordering::SeqCst), 0);
        let child = factory.child.lock().unwrap().clone().unwrap();
        // A later parent close consumes the actual retained owner or actual
        // Rejected+None proof, despite the truthful failed spawn generation.
        assert_eq!(
            timeout(BOUND, world.close(&root)).await.unwrap(),
            Ok(()),
            "{output:?}"
        );
        let closes = if matches!(output, FactoryOutput::RejectedNone) {
            0
        } else {
            1
        };
        assert_eq!(factory.resources.closes.load(Ordering::SeqCst), closes);
        assert_eq!(
            world.coordinator.lifetime_state(&child),
            Some(LifetimeState::Closed)
        );
        assert_eq!(
            timeout(BOUND, world.coordinator.spawn(command))
                .await
                .unwrap(),
            expected
        );
        assert_eq!(
            factory.calls.load(Ordering::SeqCst),
            1,
            "retained admitted retry is cached"
        );
        let saved = world.store.read().await.unwrap();
        assert_eq!(saved.spawns.len(), 1);
        assert!(saved.settlements.iter().any(|row| row.target == child));
    }
}

#[tokio::test]
async fn ready_submit_receipt_is_retained_before_drop_and_retry_stays_failed() {
    for submit_fault in [Fault::Constructor, Fault::Poll, Fault::ReadyDrop] {
        let (factory, release) =
            output_factory(FactoryOutput::Prepared, Fault::Clean, submit_fault);
        let world = with_factory(factory.clone());
        let root = world.root().await;
        world.bind_root(&root);
        let command = world.command(&root, "ready-submit", "task");
        release.send(()).unwrap();
        let expected = Err(OwnershipFailure::Submission(PortFailure::Uncertain));
        assert_eq!(
            timeout(BOUND, world.coordinator.spawn(command.clone()))
                .await
                .unwrap(),
            expected
        );
        assert_eq!(factory.submit.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            factory.submit.drops.load(Ordering::SeqCst),
            if matches!(submit_fault, Fault::Constructor) {
                0
            } else {
                1
            }
        );
        let saved = world.store.read().await.unwrap();
        let row = saved
            .spawns
            .iter()
            .find(|row| row.binding.request_id == command.request_id)
            .unwrap();
        assert_eq!(
            row.progress.known(),
            if matches!(submit_fault, Fault::ReadyDrop) {
                KnownMilestone::TaskAdmitted {
                    receipt: TaskReceiptId::new("ready-drop-receipt").unwrap(),
                }
            } else {
                KnownMilestone::Attached
            }
        );
        assert_eq!(timeout(BOUND, world.close(&root)).await.unwrap(), Ok(()));
        assert_eq!(
            timeout(BOUND, world.coordinator.spawn(command))
                .await
                .unwrap(),
            expected
        );
        assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
        assert_eq!(factory.submit.calls.load(Ordering::SeqCst), 1);
        assert_eq!(factory.resources.closes.load(Ordering::SeqCst), 1);
    }
}

struct ReleasedDropResources {
    fault: Mutex<Option<Fault>>,
    closes: AtomicUsize,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
    entered: Arc<Notify>,
    drops: Arc<AtomicUsize>,
}
impl ChildResources for ReleasedDropResources {
    fn close<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        _cause: &'life1 LifetimeCause,
        _initiator: &'life2 Initiator,
    ) -> Pin<Box<dyn Future<Output = ResourceReport> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        self.closes.fetch_add(1, Ordering::SeqCst);
        let selected = self.fault.lock().unwrap().take().unwrap_or(Fault::Clean);
        if matches!(selected, Fault::Constructor) {
            fault(false);
        }
        Box::pin(OutputFuture {
            output: Some(released()),
            gate: self.gate.lock().unwrap().take(),
            entered: self.entered.clone(),
            entered_once: false,
            fault: selected,
            drops: self.drops.clone(),
        })
    }
}
fn close_command(root: &AgentLifetimeId) -> CloseCommand {
    CloseCommand {
        lifetime: root.clone(),
        cause: LifetimeCause::HostClose,
        initiator: Initiator::Host(actor("close")),
        external_attachment: false,
        timeout: None,
    }
}
struct CountWake(AtomicUsize);
impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn released_ready_drop_preserves_failed_old_waiter_after_successful_retry() {
    let world = World::new(1);
    let root = world.root().await;
    let (release, gate) = oneshot::channel();
    let resources = Arc::new(ReleasedDropResources {
        fault: Mutex::new(Some(Fault::ReadyDrop)),
        closes: AtomicUsize::new(0),
        gate: Mutex::new(Some(gate)),
        entered: Arc::new(Notify::new()),
        drops: Arc::new(AtomicUsize::new(0)),
    });
    world
        .coordinator
        .bind_resources(root.clone(), resources.clone())
        .unwrap();
    let coordinator = world.coordinator.clone();
    let selected = close_command(&root);
    let first = tokio::spawn(async move { coordinator.end_lifetime(selected).await });
    timeout(BOUND, resources.entered.notified()).await.unwrap();
    // Pin and poll a second caller while the physical future is gated. Keep it
    // unpolled after notification until generation 2 has completed successfully.
    let wake = Arc::new(CountWake(AtomicUsize::new(0)));
    let mut old_waiter = Box::pin(world.coordinator.end_lifetime(close_command(&root)));
    assert!(old_waiter
        .as_mut()
        .poll(&mut Context::from_waker(&Waker::from(wake.clone())))
        .is_pending());
    release.send(()).unwrap();
    assert_eq!(
        timeout(BOUND, first).await.unwrap().unwrap(),
        Err(OwnershipFailure::Incomplete)
    );
    assert!(wake.0.load(Ordering::SeqCst) > 0);
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
    assert_eq!(timeout(BOUND, world.close(&root)).await.unwrap(), Ok(()));
    assert_eq!(
        timeout(BOUND, old_waiter).await.unwrap(),
        Err(OwnershipFailure::Incomplete)
    );
    assert_eq!(
        resources.closes.load(Ordering::SeqCst),
        1,
        "Released is not physically closed again"
    );
    assert_eq!(resources.drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        world.coordinator.lifetime_state(&root),
        Some(LifetimeState::Closed)
    );
}

#[tokio::test]
async fn resource_constructor_and_poll_faults_do_not_infer_release_or_absence() {
    for selected in [Fault::Constructor, Fault::Poll] {
        let world = World::new(1);
        let root = world.root().await;
        let resources = Arc::new(ReleasedDropResources {
            fault: Mutex::new(Some(selected)),
            closes: AtomicUsize::new(0),
            gate: Mutex::new(None),
            entered: Arc::new(Notify::new()),
            drops: Arc::new(AtomicUsize::new(0)),
        });
        world
            .coordinator
            .bind_resources(root.clone(), resources.clone())
            .unwrap();
        assert_eq!(
            timeout(BOUND, world.close(&root)).await.unwrap(),
            Err(OwnershipFailure::Incomplete)
        );
        assert_eq!(
            world.coordinator.lifetime_state(&root),
            Some(LifetimeState::Closing)
        );
        assert!(world.store.read().await.unwrap().settlements.is_empty());
        assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
        timeout(BOUND, world.close(&root)).await.unwrap().unwrap();
        assert_eq!(resources.closes.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn ready_submit_rejection_survives_future_drop_and_is_cached() {
    let (mut factory, release) =
        output_factory(FactoryOutput::Prepared, Fault::Clean, Fault::ReadyDrop);
    Arc::get_mut(&mut Arc::get_mut(&mut factory).unwrap().submit)
        .unwrap()
        .rejected = true;
    let world = with_factory(factory.clone());
    let root = world.root().await;
    world.bind_root(&root);
    let command = world.command(&root, "ready-submit-rejected", "task");
    release.send(()).unwrap();
    let expected = Err(OwnershipFailure::Submission(PortFailure::Rejected));
    assert_eq!(
        timeout(BOUND, world.coordinator.spawn(command.clone()))
            .await
            .unwrap(),
        expected
    );
    let saved = world.store.read().await.unwrap();
    let progress = &saved
        .spawns
        .iter()
        .find(|row| row.binding.request_id == command.request_id)
        .unwrap()
        .progress;
    assert!(!matches!(
        progress.known(),
        KnownMilestone::TaskAdmitted { .. }
    ));
    assert_eq!(timeout(BOUND, world.close(&root)).await.unwrap(), Ok(()));
    assert_eq!(
        timeout(BOUND, world.coordinator.spawn(command))
            .await
            .unwrap(),
        expected
    );
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    assert_eq!(factory.submit.calls.load(Ordering::SeqCst), 1);
    assert_eq!(factory.submit.drops.load(Ordering::SeqCst), 1);
    assert_eq!(factory.resources.closes.load(Ordering::SeqCst), 1);
}

struct EscapingSubmit {
    recovery_fault: Arc<std::sync::atomic::AtomicBool>,
    drops: Arc<AtomicUsize>,
}
#[async_trait]
impl InitialSubmit for EscapingSubmit {
    async fn submit(
        &self,
        _task: &str,
        _request: &SpawnRequestId,
    ) -> Result<TaskReceiptId, PortFailure> {
        panic!("factory Drop fault must prevent submission");
    }
}
impl Drop for EscapingSubmit {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        self.recovery_fault.store(true, Ordering::SeqCst);
        panic!("retained Ready submit handle destructor escapes inner recovery");
    }
}
struct EscapingFactory {
    resources: Arc<ScriptResources>,
    recovery_fault: Arc<std::sync::atomic::AtomicBool>,
    submit_drops: Arc<AtomicUsize>,
    calls: AtomicUsize,
}
impl ChildFactory for EscapingFactory {
    fn prepare<'life0, 'async_trait>(
        &'life0 self,
        _request: PrepareRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PreparedChild, PrepareFailure>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(OutputFuture {
            output: Some(Ok(PreparedChild {
                resources: self.resources.clone(),
                submit: Arc::new(EscapingSubmit {
                    recovery_fault: self.recovery_fault.clone(),
                    drops: self.submit_drops.clone(),
                }),
            })),
            gate: None,
            entered: Arc::new(Notify::new()),
            entered_once: false,
            fault: Fault::ReadyDrop,
            drops: Arc::new(AtomicUsize::new(0)),
        })
    }
}
struct FaultingRecoveryStore {
    memory: Arc<MemoryOwnershipStore>,
    armed: Arc<std::sync::atomic::AtomicBool>,
    faults: AtomicUsize,
}
impl OwnershipStore for FaultingRecoveryStore {
    fn write<'s, 'v, 'f>(
        &'s self,
        snapshot: &'v OwnershipSnapshot,
    ) -> Pin<Box<dyn Future<Output = Result<(), PortFailure>> + Send + 'f>>
    where
        's: 'f,
        'v: 'f,
        Self: 'f,
    {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.faults.fetch_add(1, Ordering::SeqCst);
            panic!("outer recovery store constructor");
        }
        Box::pin(self.memory.write(snapshot))
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
async fn escaping_ready_handle_drop_and_recovery_fault_still_publish_cached_terminal_result() {
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let resources = Arc::new(ScriptResources {
        closes: AtomicUsize::new(0),
        report: released(),
        hold: None,
    });
    let factory = Arc::new(EscapingFactory {
        resources: resources.clone(),
        recovery_fault: armed.clone(),
        submit_drops: Arc::new(AtomicUsize::new(0)),
        calls: AtomicUsize::new(0),
    });
    let mut world = with_factory(factory.clone());
    let store = Arc::new(FaultingRecoveryStore {
        memory: world.store.clone(),
        armed,
        faults: AtomicUsize::new(0),
    });
    world.coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: store.clone(),
        audit: world.audit.clone(),
        factory: factory.clone(),
        room: Arc::new(LiveCapacity::new(1)),
    });
    let root = world.root().await;
    world.bind_root(&root);
    let command = world.command(&root, "escaping-ready-handle", "task");
    let expected = Err(OwnershipFailure::Startup(PortFailure::Uncertain));
    assert_eq!(
        timeout(BOUND, world.coordinator.spawn(command.clone()))
            .await
            .unwrap(),
        expected
    );
    assert_eq!(factory.submit_drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.faults.load(Ordering::SeqCst),
        1,
        "bounded outer recovery actually faults"
    );
    let saved = world.store.read().await.unwrap();
    assert_eq!(saved.spawns[0].progress.known(), KnownMilestone::Prepared);
    assert!(
        saved.settlements.is_empty(),
        "a destructor fault cannot mint absence"
    );
    assert_eq!(
        timeout(BOUND, world.coordinator.spawn(command))
            .await
            .unwrap(),
        expected
    );
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);
    assert_eq!(timeout(BOUND, world.close(&root)).await.unwrap(), Ok(()));
    assert_eq!(resources.closes.load(Ordering::SeqCst), 1);
}
