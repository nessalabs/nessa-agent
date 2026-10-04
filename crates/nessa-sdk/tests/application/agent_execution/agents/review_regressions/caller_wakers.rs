//! A caller's panicking `Waker` must stay inside that caller's wait.
//!
//! Rows of "Caller wakers" in docs/agent_execution/lifecycle.md. Every test
//! runs its journey once with a well-behaved waker as the positive
//! counterpart, then with one that panics on its first wake, except the two
//! multi-thread child processes: their clean passes are the sibling tests
//! `panicking_close_waiter_does_not_interrupt_close` and
//! `panicking_attachment_waiter_leaves_the_attachment_attached`.
use super::*;
use nessa_sdk::application::agent_execution::permissions::{
    ApprovalAttribution, ApprovalBasis, PermissionAnswer, PermissionCancellationRequest,
    QuestionAnswer,
};
use nessa_sdk::application::agent_execution::providers::{
    ApprovalMode, ProviderOpenError, ProviderOpenRequest,
};
use nessa_sdk::domain::agent_execution::permissions::{
    CustomPermissionCancellationReason, PermissionCancellationReason, PermissionId,
    PermissionOptionId,
};
use nessa_sdk::domain::agent_execution::questions::QuestionId;
use nessa_sdk::domain::model_metadata::value_objects::EffortLevel;
use std::{
    io,
    pin::Pin,
    task::{Context, Wake, Waker},
};

type ReceiptWait = Pin<Box<dyn Future<Output = Result<ExecutionOutcome, AgentError>> + Send>>;

/// Same-submission retries that join the original receipt. Each is a separate
/// waiting consumer, not another admitted work owner.
const HEALTHY_WAITERS: usize = 16;

/// What the caller's waker does on its first wake.
#[derive(Clone, Copy, Debug)]
enum Fault {
    None,
    Panic,
    /// Panics with a payload whose own drop panics.
    PanicWithPanickingPayload,
    /// Panics with a payload whose drop panics with a payload whose drop
    /// panics again.
    PanicWithTwicePanickingPayload,
}

struct PanickingDrop;
impl Drop for PanickingDrop {
    fn drop(&mut self) {
        panic!("issue431 panic payload drop");
    }
}
struct TwicePanickingDrop;
impl Drop for TwicePanickingDrop {
    fn drop(&mut self) {
        std::panic::panic_any(PanickingDrop);
    }
}

struct CallerWake {
    calls: AtomicUsize,
    observed: Mutex<Option<oneshot::Sender<()>>>,
    fault: Fault,
}
impl Wake for CallerWake {
    fn wake(self: Arc<Self>) {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        // Signal the test before the fault, with no fixture lock held. The
        // callback never polls the wait it belongs to.
        let observed = self.observed.lock().unwrap().take();
        if let Some(observed) = observed {
            let _ = observed.send(());
        }
        if first {
            match self.fault {
                Fault::None => {}
                Fault::Panic => panic!("issue431 caller waker"),
                Fault::PanicWithPanickingPayload => std::panic::panic_any(PanickingDrop),
                Fault::PanicWithTwicePanickingPayload => std::panic::panic_any(TwicePanickingDrop),
            }
        }
    }
}
fn caller_wake(fault: Fault) -> (Arc<CallerWake>, oneshot::Receiver<()>) {
    let (observed, notification) = oneshot::channel();
    let caller = Arc::new(CallerWake {
        calls: AtomicUsize::new(0),
        observed: Mutex::new(Some(observed)),
        fault,
    });
    (caller, notification)
}
fn panics(fault: bool) -> Fault {
    if fault {
        Fault::Panic
    } else {
        Fault::None
    }
}

struct HealthyCallerWake(AtomicUsize);
impl Wake for HealthyCallerWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn register<T>(wait: &mut Pin<Box<dyn Future<Output = T> + Send + '_>>, waker: Waker) {
    assert!(wait
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
}

const BOUND: Duration = Duration::from_secs(3);

/// The warning `contain_caller_wake` logs, with the wait's identity.
const CONTAINED: &str = "a caller's waker panicked; its wait keeps its result";

#[derive(Clone, Default)]
struct Warnings(Arc<Mutex<Vec<u8>>>);
impl io::Write for Warnings {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Warnings {
    /// Captures warnings logged on this thread until the guard drops. Run
    /// the journey on a current-thread runtime so the SDK's tasks log here,
    /// and only in a child process running one test: tracing caches callsite
    /// interest process-wide, so tests on other threads can hide an event.
    fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let warnings = Self::default();
        let writer = warnings.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            // CI's package selection unifies tracing-subscriber's `ansi`
            // feature in, which would colour field names in the capture.
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        (warnings, tracing::subscriber::set_default(subscriber))
    }
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
    }
}
/// Asserts the containment warning names `waiter` exactly when `fault`
/// panicked, and is absent otherwise.
fn assert_contained(logged: &str, fault: Fault, waiter: &str) {
    match fault {
        Fault::None => assert!(
            !logged.contains(CONTAINED),
            "no panic, no warning: {logged}"
        ),
        _ => assert!(
            logged.contains(CONTAINED) && logged.contains(&format!("waiter={waiter}")),
            "the warning names {waiter}: {logged}"
        ),
    }
}

#[tokio::test]
async fn panicking_receipt_consumer_preserves_independent_queued_work() {
    for fault in [Fault::None, Fault::Panic, Fault::PanicWithPanickingPayload] {
        let (agent, backend, storage) = probe(false).await;
        let (release, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let original = agent.enqueue(input("original"), actor()).await.unwrap();
        timeout(BOUND, backend.executing.notified()).await.unwrap();
        let tail = agent
            .enqueue(input("independent-tail"), actor())
            .await
            .unwrap();
        let mut retries = Vec::new();
        for _ in 0..HEALTHY_WAITERS {
            let retry = agent.enqueue(input("original"), actor()).await.unwrap();
            assert_eq!(original.id(), retry.id());
            retries.push(retry);
        }

        let (caller, notification) = caller_wake(fault);
        let mut original: ReceiptWait = Box::pin(original.wait());
        register(&mut original, Waker::from(caller.clone()));
        let mut healthy = Vec::new();
        for retry in retries {
            let counter = Arc::new(HealthyCallerWake(AtomicUsize::new(0)));
            let mut wait: ReceiptWait = Box::pin(retry.wait());
            register(&mut wait, Waker::from(counter.clone()));
            healthy.push((counter, wait));
        }
        release.send(()).unwrap();
        // Do not await any receipt before this witness: that would replace its
        // custom caller registration before publication.
        timeout(BOUND, notification).await.unwrap().unwrap();
        let tail_result = timeout(BOUND, tail.wait()).await;
        // The tail settled after the original's publication returned, so every
        // registered sibling has been notified by now if it ever will be.
        let unwoken = healthy
            .iter()
            .filter(|(counter, _)| counter.0.load(Ordering::SeqCst) == 0)
            .count();

        assert_eq!(caller.calls.load(Ordering::SeqCst), 1, "{fault:?}");
        assert_eq!(
            backend.executions.load(Ordering::SeqCst),
            2,
            "{fault:?}: an independently admitted tail must dispatch after another receipt consumer panics"
        );
        assert_eq!(
            tail_result.expect("the independent tail must settle"),
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(
            unwoken, 0,
            "{fault:?}: every registered sibling waiter must be notified despite one consumer's panic"
        );
        assert_eq!(
            timeout(BOUND, original).await.unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        for (_, wait) in healthy {
            assert_eq!(
                timeout(BOUND, wait).await.unwrap(),
                Ok(ExecutionOutcome::Completed)
            );
        }
        let saved = storage.snapshot();
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
        assert_eq!(saved.invocations.len(), 2);
        for (record, id) in saved
            .invocations
            .iter()
            .zip(["original", "independent-tail"])
        {
            assert_eq!(record.request.execution_id.as_str(), id);
            assert_eq!(record.result, Some(Ok(ExecutionOutcome::Completed)));
            assert_eq!(
                record.scheduling.last().unwrap().stage,
                InvocationStage::Settled
            );
        }
    }
}

#[tokio::test]
async fn panicking_receipt_consumer_does_not_fail_its_withdrawal() {
    for fault in [false, true] {
        let (agent, backend, storage) = probe(false).await;
        let (release, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let active = agent.enqueue(input("active"), actor()).await.unwrap();
        timeout(BOUND, backend.executing.notified()).await.unwrap();
        let withdrawn = agent.enqueue(input("withdrawn"), actor()).await.unwrap();
        let (caller, notification) = caller_wake(panics(fault));
        let mut withdrawn: ReceiptWait = Box::pin(withdrawn.wait());
        register(&mut withdrawn, Waker::from(caller.clone()));

        let removal = timeout(
            BOUND,
            agent.remove_queued(ExecutionId::new("withdrawn").unwrap(), actor()),
        )
        .await
        .unwrap();
        timeout(BOUND, notification).await.unwrap().unwrap();
        assert_eq!(
            removal,
            Ok(QueueRemoval::Removed),
            "a consumer's wake panic must not turn an applied withdrawal into an error"
        );
        assert_eq!(caller.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            timeout(BOUND, withdrawn).await.unwrap(),
            Err(AgentError::Closed)
        );
        release.send(()).unwrap();
        assert_eq!(
            timeout(BOUND, active.wait()).await.unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
        let saved = storage.snapshot();
        let record = saved
            .invocations
            .iter()
            .find(|record| record.request.execution_id.as_str() == "withdrawn")
            .unwrap();
        let last = record.scheduling.last().unwrap();
        assert_eq!(last.stage, InvocationStage::Cancelled);
        assert_eq!(last.cause, SchedulingCause::Withdrawn);
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn panicking_event_subscriber_does_not_stop_the_invocation_that_published() {
    for fault in [false, true] {
        let (agent, backend, _storage) = probe(false).await;
        let mut updates = agent.subscribe();
        let (caller, notification) = caller_wake(panics(fault));
        let mut next: Pin<Box<dyn Future<Output = _> + Send + '_>> = Box::pin(updates.next());
        register(&mut next, Waker::from(caller.clone()));

        let first = agent.enqueue(input("first"), actor()).await.unwrap();
        let tail = agent.enqueue(input("tail"), actor()).await;
        timeout(BOUND, notification).await.unwrap().unwrap();
        let first = timeout(BOUND, first.wait()).await.unwrap();
        let tail = match tail {
            Ok(tail) => timeout(BOUND, tail.wait()).await.unwrap(),
            Err(error) => Err(error),
        };

        assert_eq!(caller.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            first,
            Ok(ExecutionOutcome::Completed),
            "a subscriber's wake panic must not fail the invocation that published"
        );
        assert_eq!(tail, Ok(ExecutionOutcome::Completed));
        assert_eq!(backend.executions.load(Ordering::SeqCst), 2);
        assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Attached);
        // The update that woke the subscriber is still there to read.
        let event = timeout(BOUND, next).await.unwrap().unwrap().unwrap();
        assert_eq!(event.execution_id().as_str(), "first");
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn panicking_attachment_cancellation_waiter_does_not_interrupt_close() {
    for fault in [false, true] {
        let (agent, _backend, _storage) = probe(false).await;
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        let (caller, notification) = caller_wake(panics(fault));
        let mut cancelled: Pin<Box<dyn Future<Output = ()> + Send>> =
            Box::pin(authorization.cancellation().wait());
        register(&mut cancelled, Waker::from(caller.clone()));

        let closed = timeout(BOUND, agent.close(actor())).await.unwrap();
        timeout(BOUND, notification).await.unwrap().unwrap();
        assert!(
            closed.is_ok(),
            "a cancellation waiter's wake panic must not interrupt close: {closed:?}"
        );
        assert_eq!(caller.calls.load(Ordering::SeqCst), 1);
        timeout(BOUND, cancelled).await.unwrap();
        drop(authorization);
        // The lifecycle stays usable: its state was not left mid-update.
        assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Absent);
        reattach_after_explicit_close(&agent).await;
        assert_eq!(
            timeout(BOUND, agent.invoke(input("after"), actor()))
                .await
                .unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
    }
}

/// Opens normally once, then stalls the next open with its stop wait
/// registered under the caller's waker until a stop is published.
struct StallingOpen {
    inner: ProbeFactory,
    opens: AtomicUsize,
    fault: bool,
    caller: Mutex<Option<Arc<CallerWake>>>,
    registered: Notify,
}
impl AgentProvider for StallingOpen {
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        if self.opens.fetch_add(1, Ordering::SeqCst) == 0 {
            return self.inner.open(request);
        }
        Box::pin(async move {
            let (_, _, mut control) = request.into_parts();
            let (caller, notification) = caller_wake(panics(self.fault));
            *self.caller.lock().unwrap() = Some(caller.clone());
            let mut stop: Pin<Box<dyn Future<Output = _> + Send + '_>> = Box::pin(control.wait());
            register(&mut stop, Waker::from(caller));
            self.registered.notify_one();
            let _ = notification.await;
            let request = stop.await;
            assert!(request.is_some(), "the stop was published");
            Err(ProviderOpenError::no_resources(AgentError::Closed))
        })
    }
}

#[tokio::test]
async fn panicking_provider_open_stop_waiter_does_not_interrupt_close() {
    for fault in [false, true] {
        let (inner, _backend) = probe_factory(false);
        let provider = Arc::new(StallingOpen {
            inner,
            opens: AtomicUsize::new(0),
            fault,
            caller: Mutex::new(None),
            registered: Notify::new(),
        });
        let storage = MemoryStorage::default();
        let agent = attached_agent(provider.clone(), storage.manager().await)
            .await
            .unwrap();
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        let attaching = agent.start_attachment(authorization).unwrap();
        timeout(BOUND, provider.registered.notified())
            .await
            .unwrap();

        let closed = timeout(BOUND, agent.close(actor())).await.unwrap();
        assert!(
            closed.is_ok(),
            "a provider's stop-wait wake panic must not interrupt close: {closed:?}"
        );
        let caller = provider.caller.lock().unwrap().clone().unwrap();
        assert_eq!(caller.calls.load(Ordering::SeqCst), 1);
        assert!(timeout(BOUND, attaching.wait()).await.unwrap().is_err());
        // The lifecycle stays usable: its state was not left mid-update.
        assert_ne!(agent.attachment_status().phase(), AttachmentPhase::Attached);
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
    }
}

/// Public waits that queue behind an SDK-owned Tokio mutex.
#[derive(Clone, Copy, Debug)]
enum LockWaiter {
    QueuedIds,
    IdleForApprovalChange,
    SetApprovalMode,
    CommittedSnapshot,
}

#[tokio::test]
async fn panicking_lock_waiter_does_not_fail_the_admission_that_released_it() {
    for waiter in [
        LockWaiter::QueuedIds,
        LockWaiter::IdleForApprovalChange,
        LockWaiter::SetApprovalMode,
        LockWaiter::CommittedSnapshot,
    ] {
        for fault in [false, true] {
            let (agent, backend, storage) = probe(false).await;
            // Admission holds the scheduler lock, and the manager its evidence
            // lock, across this save.
            let (saving, release) = storage.pause_next_save();
            let admitting = {
                let agent = agent.clone();
                tokio::spawn(async move { agent.enqueue(input("queued"), actor()).await })
            };
            timeout(BOUND, saving).await.unwrap().unwrap();

            let (caller, notification) = caller_wake(panics(fault));
            let mut waiting: Pin<Box<dyn Future<Output = ()> + Send + '_>> = match waiter {
                LockWaiter::QueuedIds => Box::pin(async {
                    agent.queued_ids().await;
                }),
                LockWaiter::IdleForApprovalChange => Box::pin(async {
                    agent.idle_for_approval_change().await;
                }),
                LockWaiter::SetApprovalMode => Box::pin(async {
                    let _ = agent.set_approval_mode(ApprovalMode::Ask).await;
                }),
                LockWaiter::CommittedSnapshot => Box::pin(async {
                    agent.session_manager().snapshot().await;
                }),
            };
            register(&mut waiting, Waker::from(caller.clone()));
            release.send(()).unwrap();
            timeout(BOUND, notification).await.unwrap().unwrap();

            // Tokio hands a released lock to its next waiter, so the waiting
            // call holds the lock until it is polled again.
            timeout(BOUND, waiting).await.unwrap();
            let admitted = timeout(BOUND, admitting).await.unwrap().unwrap();
            let admitted = match admitted {
                Ok(admitted) => admitted,
                Err(error) => panic!(
                    "{waiter:?}: a lock waiter's wake panic must not fail the admission: {error:?}"
                ),
            };
            assert_eq!(
                timeout(BOUND, admitted.wait()).await.unwrap(),
                Ok(ExecutionOutcome::Completed),
                "{waiter:?}"
            );
            assert_eq!(caller.calls.load(Ordering::SeqCst), 1, "{waiter:?}");
            assert_eq!(backend.executions.load(Ordering::SeqCst), 1, "{waiter:?}");
            timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
        }
    }
}

#[tokio::test]
async fn panicking_attachment_waiter_leaves_the_attachment_attached() {
    for fault in [Fault::None, Fault::Panic, Fault::PanicWithPanickingPayload] {
        let (agent, _backend, _storage) = probe(false).await;
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        let (caller, notification) = caller_wake(fault);
        let mut attaching: Pin<Box<dyn Future<Output = _> + Send>> =
            Box::pin(agent.start_attachment(authorization).unwrap().wait());
        register(&mut attaching, Waker::from(caller.clone()));
        timeout(BOUND, notification).await.unwrap().unwrap();

        assert_eq!(caller.calls.load(Ordering::SeqCst), 1);
        assert_eq!(timeout(BOUND, attaching).await.unwrap(), Ok(()));
        assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Attached);
        let queued = agent.enqueue(input("after"), actor()).await.unwrap();
        assert_eq!(
            timeout(BOUND, queued.wait()).await.unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
    }
}

/// Close runs on its own task and Tokio wakes a `JoinHandle` waiter inside
/// its own `catch_unwind`, so a plain panic passes without containment too.
/// The double fault below is what needs it.
#[tokio::test]
async fn panicking_close_waiter_does_not_interrupt_close() {
    for fault in [false, true] {
        let (agent, _backend, _storage) = probe(false).await;
        let (caller, notification) = caller_wake(panics(fault));
        let mut closing: Pin<Box<dyn Future<Output = _> + Send + '_>> =
            Box::pin(agent.close(actor()));
        register(&mut closing, Waker::from(caller.clone()));
        timeout(BOUND, notification).await.unwrap().unwrap();

        assert_eq!(caller.calls.load(Ordering::SeqCst), 1);
        assert!(timeout(BOUND, closing).await.unwrap().is_ok());
        assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Absent);
        reattach_after_explicit_close(&agent).await;
        assert_eq!(
            timeout(BOUND, agent.invoke(input("after"), actor()))
                .await
                .unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
    }
}

const CHILD_ENV: &str = "NESSA_ISSUE431_CHILD";

/// Runs `child` (an ignored test in this module) in a separate process and
/// fails unless it passes. A process abort there fails this test instead of
/// ending the test binary.
fn run_in_child_process(child: &str) {
    run_case_in_child_process(child, "");
}
/// As [`run_in_child_process`], passing `case` to the child in [`CHILD_ENV`].
fn run_case_in_child_process(child: &str, case: &str) {
    let name = format!("{}::{child}", module_path!().split_once("::").unwrap().1);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--ignored", "--test-threads=1"])
        .env(CHILD_ENV, case)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{child} {case} must not abort or fail: {:?}\n{stdout}\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran {name}: {stdout}"
    );
}
fn multi_thread_child_runtime() -> tokio::runtime::Runtime {
    assert!(
        std::env::var_os(CHILD_ENV).is_some(),
        "run through run_in_child_process"
    );
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// Tokio catches a panicking `JoinHandle` waker but drops the payload outside
/// that catch. A payload whose drop panics then aborts the process on a
/// multi-thread runtime.
#[test]
fn panicking_payload_close_waiter_does_not_abort_the_runtime() {
    run_in_child_process("double_fault_close_waiter_child");
}

#[test]
#[ignore = "run in a child process by panicking_payload_close_waiter_does_not_abort_the_runtime"]
fn double_fault_close_waiter_child() {
    multi_thread_child_runtime().block_on(async {
        let (agent, backend, _storage) = probe(false).await;
        // On worker threads the spawned close could finish before the first
        // poll; holding the provider close keeps it running until the
        // caller's waker is registered.
        let (release, gate) = oneshot::channel();
        *backend.close_gate.lock().unwrap() = Some(gate);
        let (caller, notification) = caller_wake(Fault::PanicWithPanickingPayload);
        let mut closing: Pin<Box<dyn Future<Output = _> + Send + '_>> =
            Box::pin(agent.close(actor()));
        register(&mut closing, Waker::from(caller.clone()));
        release.send(()).unwrap();
        timeout(BOUND, notification).await.unwrap().unwrap();
        assert_eq!(caller.calls.load(Ordering::SeqCst), 1);
        assert!(timeout(BOUND, closing).await.unwrap().is_ok());
        assert!(backend.close_gate.lock().unwrap().is_none(), "close held");
        reattach_after_explicit_close(&agent).await;
        assert_eq!(
            timeout(BOUND, agent.invoke(input("after"), actor()))
                .await
                .unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
    });
}

/// The attachment task's own panic is stored as its `JoinError` and dropped
/// inside Tokio's catch at completion; a second panic from that drop is
/// caught, and its payload dropped outside the catch. A third panic there
/// aborts the process on a multi-thread runtime.
#[test]
fn twice_panicking_payload_attachment_waiter_does_not_abort_the_runtime() {
    run_in_child_process("triple_fault_attachment_waiter_child");
}

#[test]
#[ignore = "run in a child process by twice_panicking_payload_attachment_waiter_does_not_abort_the_runtime"]
fn triple_fault_attachment_waiter_child() {
    multi_thread_child_runtime().block_on(async {
        let (agent, backend, _storage) = probe(false).await;
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        // On worker threads the attachment task could finish before the
        // first poll; holding the provider open keeps it running until the
        // caller's waker is registered.
        let (release, gate) = oneshot::channel();
        *backend.open_gate.lock().unwrap() = Some(gate);
        let (caller, notification) = caller_wake(Fault::PanicWithTwicePanickingPayload);
        let mut attaching: Pin<Box<dyn Future<Output = _> + Send>> =
            Box::pin(agent.start_attachment(authorization).unwrap().wait());
        register(&mut attaching, Waker::from(caller.clone()));
        release.send(()).unwrap();
        timeout(BOUND, notification).await.unwrap().unwrap();
        assert_eq!(caller.calls.load(Ordering::SeqCst), 1);
        assert_eq!(timeout(BOUND, attaching).await.unwrap(), Ok(()));
        assert!(backend.open_gate.lock().unwrap().is_none(), "open held");
        assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Attached);
        assert_eq!(
            timeout(BOUND, agent.invoke(input("after"), actor()))
                .await
                .unwrap(),
            Ok(ExecutionOutcome::Completed)
        );
        timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
    });
}

/// Public operations that spawn their owner and await its `JoinHandle`.
#[derive(Clone, Copy, Debug)]
enum JoinedOperation {
    Invoke,
    Enqueue,
    EnqueueSteering,
    Steer,
    ReorderQueued,
    RemoveQueued,
    Close,
    SetEffortLevel,
    AnswerPermission,
    CancelPermission,
    AnswerQuestion,
}
const JOINED_OPERATIONS: [JoinedOperation; 11] = [
    JoinedOperation::Invoke,
    JoinedOperation::Enqueue,
    JoinedOperation::EnqueueSteering,
    JoinedOperation::Steer,
    JoinedOperation::ReorderQueued,
    JoinedOperation::RemoveQueued,
    JoinedOperation::Close,
    JoinedOperation::SetEffortLevel,
    JoinedOperation::AnswerPermission,
    JoinedOperation::CancelPermission,
    JoinedOperation::AnswerQuestion,
];
type Joined<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
impl JoinedOperation {
    /// Starts the operation and names the wait it logs on containment.
    fn start<'a>(self, agent: &'a Agent) -> (Joined<'a>, String) {
        let session = agent.session_manager().id().as_str().to_owned();
        let execution = ExecutionId::new("joined").unwrap();
        match self {
            Self::Invoke => (
                Box::pin(async {
                    let _ = agent.invoke(input("joined"), actor()).await;
                }),
                "invocation joined".into(),
            ),
            Self::Enqueue => (
                Box::pin(async {
                    let _ = agent.enqueue(input("joined"), actor()).await;
                }),
                "admission of joined".into(),
            ),
            Self::EnqueueSteering => (
                Box::pin(async {
                    let _ = agent.enqueue_steering(input("joined"), actor()).await;
                }),
                "admission of joined".into(),
            ),
            Self::Steer => (
                Box::pin(async {
                    let _ = agent.steer(input("joined"), actor()).await;
                }),
                "steering of joined".into(),
            ),
            Self::ReorderQueued => (
                Box::pin(async {
                    let _ = agent.reorder_queued(Vec::new(), actor()).await;
                }),
                format!("queue reorder of session {session}"),
            ),
            Self::RemoveQueued => (
                Box::pin(async move {
                    let _ = agent.remove_queued(execution, actor()).await;
                }),
                "removal of queued joined".into(),
            ),
            Self::Close => (
                Box::pin(async {
                    let _ = agent.close(actor()).await;
                }),
                format!("close of session {session}"),
            ),
            Self::SetEffortLevel => (
                Box::pin(async {
                    let level = EffortLevel::new("high".into()).unwrap();
                    let _ = agent.set_effort_level(level, actor()).await;
                }),
                format!("effort level change of session {session}"),
            ),
            Self::AnswerPermission => (
                Box::pin(async move {
                    let answer = PermissionAnswer {
                        attribution: ApprovalAttribution::new(actor(), ApprovalBasis::Explicit),
                        execution_id: execution,
                        id: PermissionId::new("review").unwrap(),
                        option_id: PermissionOptionId::new("allow").unwrap(),
                    };
                    let _ = agent.answer_permission(answer).await;
                }),
                format!("permission answer in session {session}"),
            ),
            Self::CancelPermission => (
                Box::pin(async move {
                    let request = PermissionCancellationRequest {
                        execution_id: execution,
                        id: PermissionId::new("review").unwrap(),
                        reason: PermissionCancellationReason::custom(
                            CustomPermissionCancellationReason::new("withdraw", "caller withdrew")
                                .unwrap(),
                        ),
                        actor: actor(),
                    };
                    let _ = agent.cancel_permission(request).await;
                }),
                format!("permission cancellation in session {session}"),
            ),
            Self::AnswerQuestion => (
                Box::pin(async move {
                    let answer = QuestionAnswer {
                        actor: actor(),
                        execution_id: execution,
                        id: QuestionId::new("ask").unwrap(),
                        choices: None,
                    };
                    let _ = agent.answer_question(answer).await;
                }),
                format!("question answer in session {session}"),
            ),
        }
    }
}

/// Every spawn-and-join operation, with a waker that panics with a payload
/// whose drop panics. Tokio drops that payload outside its own catch, so
/// each case runs in a child process: an escaping panic fails that case by
/// name instead of ending this test binary.
#[test]
fn panicking_payload_waiter_of_each_joined_operation_is_contained() {
    for operation in JOINED_OPERATIONS {
        run_case_in_child_process("joined_operation_child", &format!("{operation:?}"));
    }
}

/// One case of `panicking_payload_waiter_of_each_joined_operation_is_contained`,
/// first with a waker that does not panic. A current-thread runtime cannot
/// run the spawned owner before the operation's first poll, so the caller's
/// waker is registered with the `JoinHandle` on every run; a panic escaping
/// Tokio's completion unwinds out of `block_on` and fails this child.
#[test]
#[ignore = "run in a child process by panicking_payload_waiter_of_each_joined_operation_is_contained"]
fn joined_operation_child() {
    let case = std::env::var(CHILD_ENV).expect("run through run_case_in_child_process");
    let operation = JOINED_OPERATIONS
        .into_iter()
        .find(|operation| format!("{operation:?}") == case)
        .unwrap_or_else(|| panic!("unknown case {case}"));
    let (warnings, _capture) = Warnings::capture();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for fault in [Fault::None, Fault::PanicWithPanickingPayload] {
            let (agent, _backend, _storage) = probe(false).await;
            warnings.take();
            let (caller, notification) = caller_wake(fault);
            let (mut running, waiter) = operation.start(&agent);
            register(&mut running, Waker::from(caller.clone()));
            timeout(BOUND, notification).await.unwrap().unwrap();
            timeout(BOUND, running).await.unwrap();
            assert_eq!(caller.calls.load(Ordering::SeqCst), 1, "{operation:?}");
            assert_contained(&warnings.take(), fault, &waiter);
            timeout(BOUND, agent.close(actor())).await.unwrap().unwrap();
        }
    });
}
