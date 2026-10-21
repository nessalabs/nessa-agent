//! Failed queue settlement stays complete and preserves the first stop owner.
use super::*;
use crate::application::agent_execution::{
    agents::AgentFuture,
    executions::{ExecutionAudit, QueueSettlementRecord},
    providers::{
        AgentProvider, ProviderIdentity, ProviderOpenError, ProviderOpenFuture, ProviderOpenRequest,
    },
    sessions::{
        MessageCommitClock, MessageCommitSleep, SessionChange, SessionLoad, SessionManager,
        SessionSaveGeneration, SessionSaveReceipt, SessionSaveUnit, SessionStorage,
        SessionStorageLease, StorageFuture,
    },
};
use crate::application::dto::{ModalitiesDto, ModelMetadataDto};
use crate::domain::agent_execution::{
    executions::{QueueMutation, QueueRemovalCause, SchedulingInitiator},
    prompts::{PromptText, UserMessage},
    sessions::SessionId,
};
use crate::domain::{
    effective_capabilities::value_objects::{BindingRestrictions, EffectiveCapabilities},
    model_metadata::{
        entities::ModelMetadata,
        value_objects::{Modalities, ModelFeatures},
    },
};
use crate::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{oneshot, watch};

#[path = "scheduling/audit_logging.rs"]
mod audit_logging;

#[derive(Default)]
struct SettlementPanickingAudit {
    panic_once: AtomicBool,
    settlements: Mutex<Vec<QueueSettlementRecord>>,
}

struct PausingSettlementAudit {
    settlements: Mutex<Vec<QueueSettlementRecord>>,
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

struct PendingAdmissionAudit {
    entered: Mutex<Option<oneshot::Sender<()>>>,
}

struct FirstSettlementStalls {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    attempts: Mutex<Vec<ExecutionId>>,
    stalls: usize,
}

struct LateAdmissionWrite {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    completed: Mutex<Option<oneshot::Sender<()>>>,
    attempts: AtomicUsize,
}

// A clock independent of Tokio time proves scheduling uses the supplied timer.
struct AuditClock(watch::Sender<Duration>);
impl AuditClock {
    fn new() -> Self {
        Self(watch::channel(Duration::ZERO).0)
    }
    fn advance(&self, by: Duration) {
        self.0.send_modify(|now| *now += by);
    }
}
impl MessageCommitClock for AuditClock {
    fn now(&self) -> Duration {
        *self.0.borrow()
    }
    fn sleep_until(&self, deadline: Duration) -> MessageCommitSleep {
        let mut changed = self.0.subscribe();
        Box::pin(async move {
            loop {
                if *changed.borrow() >= deadline {
                    return;
                }
                changed.changed().await.unwrap();
            }
        })
    }
    fn unix_milliseconds(&self) -> u64 {
        0
    }
}

struct DroppingAudit {
    live: Arc<AtomicUsize>,
    attempts: AtomicUsize,
}
struct PendingObservation {
    live: Arc<AtomicUsize>,
    _record: ExecutionAuditRecord,
}
impl Future for PendingObservation {
    type Output = Result<(), AgentError>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}
impl Drop for PendingObservation {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}
impl ExecutionAudit for DroppingAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.live.fetch_add(1, Ordering::SeqCst);
        Box::pin(PendingObservation {
            live: self.live.clone(),
            _record: record,
        })
    }
}

struct GatedAudit {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}
impl ExecutionAudit for GatedAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            let release = self.release.lock().unwrap().take().unwrap();
            self.entered
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            release.await.unwrap();
            Ok(())
        })
    }
}

#[derive(Clone, Copy)]
enum AuditPanic {
    Creation,
    Poll,
    Drop,
}
struct PanickingObservation(AuditPanic);
impl Future for PanickingObservation {
    type Output = Result<(), AgentError>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.0, AuditPanic::Poll) {
            panic!("audit poll");
        }
        Poll::Ready(Ok(()))
    }
}
impl Drop for PanickingObservation {
    fn drop(&mut self) {
        if matches!(self.0, AuditPanic::Drop) {
            panic!("audit drop");
        }
    }
}
impl ExecutionAudit for AuditPanic {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        if matches!(self, AuditPanic::Creation) {
            panic!("audit creation");
        }
        Box::pin(PanickingObservation(*self))
    }
}

struct CountingProvider(Arc<AtomicUsize>);
impl AgentProvider for CountingProvider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn identity(&self) -> ProviderIdentity {
        TestProvider.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) })
    }
}

struct TestProvider;

impl AgentProvider for TestProvider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("failed-pending", "fixture", "fixture").unwrap()
    }

    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities()
    }

    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) })
    }
}

fn capabilities() -> &'static EffectiveCapabilities {
    static CAPABILITIES: OnceLock<EffectiveCapabilities> = OnceLock::new();
    CAPABILITIES.get_or_init(|| {
        let text = ModalitiesDto {
            text: true,
            image: false,
            audio: false,
        };
        let model = ModelMetadata::try_from(ModelMetadataDto {
            provider: "anthropic".into(),
            model_id: "fixture".into(),
            display_name: "Fixture".into(),
            input: text,
            image_input: None,
            output: text,
            tool_use: true,
            reasoning: None,
            fast_mode: false,
            max_context_window_tokens: 1000,
            max_output_tokens: 100,
            knowledge_cutoff: "2026-01".into(),
            documentation_url: "https://example.com".into(),
        })
        .unwrap();
        let text = Modalities::new(true, false, false).unwrap();
        EffectiveCapabilities::new(
            &model,
            BindingRestrictions::new(
                ModelFeatures::new(text, text, true, false, false),
                model.limits(),
            ),
            model.limits(),
        )
        .unwrap()
    })
}

impl ExecutionAudit for PausingSettlementAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if let ExecutionAuditRecord::QueueSettled(record) = record {
                self.settlements.lock().unwrap().push(record);
                let gate = self.gate.lock().unwrap().take();
                if let Some((entered, release)) = gate {
                    entered.send(()).unwrap();
                    release.await.unwrap();
                }
            }
            Ok(())
        })
    }
}

impl ExecutionAudit for SettlementPanickingAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if let ExecutionAuditRecord::QueueSettled(record) = record {
                self.settlements.lock().unwrap().push(record);
                if !self.panic_once.swap(true, Ordering::SeqCst) {
                    panic!("first queue settlement audit panicked");
                }
            }
            Ok(())
        })
    }
}

impl ExecutionAudit for PendingAdmissionAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if matches!(record, ExecutionAuditRecord::QueueAdmitted(_)) {
                if let Some(entered) = self.entered.lock().unwrap().take() {
                    entered.send(()).unwrap();
                }
                return std::future::pending().await;
            }
            Ok(())
        })
    }
}

impl ExecutionAudit for FirstSettlementStalls {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if let ExecutionAuditRecord::QueueSettled(record) = record {
                let attempt = {
                    let mut attempts = self.attempts.lock().unwrap();
                    attempts.push(record.execution_id().clone());
                    attempts.len()
                };
                if attempt == 1 {
                    self.entered
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(())
                        .unwrap();
                }
                if attempt <= self.stalls {
                    return std::future::pending().await;
                }
            }
            Ok(())
        })
    }
}

impl ExecutionAudit for LateAdmissionWrite {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if matches!(record, ExecutionAuditRecord::QueueAdmitted(_)) {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                self.entered
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(())
                    .unwrap();
                let release = self.release.lock().unwrap().take().unwrap();
                // Match the durable adapter's independently owned physical I/O:
                // losing its async observer does not abort the started write.
                let completed = self.completed.lock().unwrap().take().unwrap();
                tokio::spawn(async move {
                    release.await.unwrap();
                    completed.send(()).unwrap();
                })
                .await
                .unwrap();
            }
            Ok(())
        })
    }
}

struct PanickingQueueStorage {
    inner: InMemoryStorage,
    mutation: Arc<Mutex<Option<QueueMutation>>>,
}

struct PanickingQueueLease {
    inner: Box<dyn SessionStorageLease>,
    mutation: Arc<Mutex<Option<QueueMutation>>>,
}

impl SessionStorage for PanickingQueueStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(PanickingQueueLease {
                inner: self.inner.open(id).await?,
                mutation: self.mutation.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}

impl SessionStorageLease for PanickingQueueLease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }

    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        let should_panic = self
            .mutation
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|mutation| {
                units.iter().flat_map(SessionSaveUnit::changes).any(|change| {
                    matches!(change, SessionChange::QueueDecision(entry) if &entry.mutation == mutation)
                })
            });
        if should_panic {
            self.mutation.lock().unwrap().take();
            return Box::pin(async { panic!("queue membership persistence panic") });
        }
        self.inner.save_changes(binding, units)
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
}

async fn prepared(storage: Arc<dyn SessionStorage>, audit: Arc<dyn ExecutionAudit>) -> Agent {
    let manager = SessionManager::open(None, storage, Arc::new(RuntimeMessageCommitClock::new()))
        .await
        .unwrap();
    Agent::prepare(Arc::new(TestProvider), manager, audit)
        .await
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn pending_admission_audit_fails_and_close_completes_after_physical_cleanup_starts() {
    let (audit_entered, audit_waiting) = oneshot::channel();
    let audit = Arc::new(PendingAdmissionAudit {
        entered: Mutex::new(Some(audit_entered)),
    });
    let clock = Arc::new(AuditClock::new());
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()), clock.clone())
        .await
        .unwrap();
    let provider_opens = Arc::new(AtomicUsize::new(0));
    let agent = Agent::prepare(
        Arc::new(CountingProvider(provider_opens.clone())),
        manager,
        audit,
    )
    .await
    .unwrap();
    let (cleanup_entered, cleanup_started) = oneshot::channel();
    agent
        .inner
        .lifecycle
        .notify_next_cleanup_start(cleanup_entered);

    let admission = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .enqueue(request("pending-admission"), Submitter::owner(actor()))
                .await
        }
    });
    audit_waiting.await.unwrap();
    let close = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(actor()).await }
    });

    cleanup_started.await.unwrap();
    assert!(!close.is_finished());
    // Runtime time alone cannot expire this separately injected clock.
    tokio::time::advance(AUDIT_OBSERVATION_TIMEOUT).await;
    assert!(!admission.is_finished());
    clock.advance(AUDIT_OBSERVATION_TIMEOUT);
    let admission = tokio::time::timeout(Duration::from_secs(1), admission)
        .await
        .expect("the injected audit deadline must release admission")
        .unwrap()
        .unwrap();
    assert_eq!(
        admission.evidence(),
        &AdmissionEvidence::Failed(
            AdmissionEvidenceFailure::new(Some(AgentError::AuditFailure), None).unwrap()
        )
    );
    assert_eq!(admission.wait().await, Err(AgentError::Closed));
    assert!(close.await.unwrap().is_ok());
    assert_eq!(provider_opens.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn stalled_first_bulk_settlement_does_not_suppress_later_audit_or_receipt() {
    let (first_entered, first_waiting) = oneshot::channel();
    let audit = Arc::new(FirstSettlementStalls {
        entered: Mutex::new(Some(first_entered)),
        attempts: Mutex::new(Vec::new()),
        stalls: 1,
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let first = agent
        .enqueue(request("bulk-stalled-first"), Submitter::owner(actor()))
        .await
        .unwrap();
    let second = agent
        .enqueue(request("bulk-attempted-second"), Submitter::owner(actor()))
        .await
        .unwrap();
    let settlement = tokio::spawn({
        let agent = agent.clone();
        async move {
            let mut scheduler = agent.inner.scheduler.lock().await;
            agent
                .settle_failed_pending(
                    &mut scheduler,
                    AgentError::Protocol("attachment failed".into()),
                )
                .await
        }
    });
    first_waiting.await.unwrap();
    tokio::time::advance(AUDIT_OBSERVATION_TIMEOUT).await;

    assert!(matches!(
        settlement.await.unwrap(),
        Err(AgentError::AuditFailure)
    ));
    assert_eq!(
        audit.attempts.lock().unwrap().as_slice(),
        &[
            ExecutionId::new("bulk-stalled-first").unwrap(),
            ExecutionId::new("bulk-attempted-second").unwrap(),
        ]
    );
    assert!(matches!(
        first.wait().await,
        Err(AgentError::MultipleOperationFailures { .. })
    ));
    assert_eq!(
        second.wait().await,
        Err(AgentError::Protocol("attachment failed".into()))
    );
}

#[tokio::test(start_paused = true)]
async fn timed_out_observer_leaves_late_physical_audit_write_owned_without_retry() {
    let (entered, waiting) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (completed, completion) = oneshot::channel();
    let audit = Arc::new(LateAdmissionWrite {
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(Some(released)),
        completed: Mutex::new(Some(completed)),
        attempts: AtomicUsize::new(0),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let observer = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .record_scheduling_audit(
                    ExecutionAuditRecord::QueueAdmitted(QueueAdmissionRecord::submitted(
                        agent.inner.manager.id().clone(),
                        ExecutionId::new("late-audit-write").unwrap(),
                        SubmissionMode::Queued,
                        actor(),
                    )),
                    "late_audit_write_test",
                )
                .await
        }
    });
    waiting.await.unwrap();
    tokio::time::advance(AUDIT_OBSERVATION_TIMEOUT).await;
    assert_eq!(
        observer.await.unwrap(),
        Err(AuditObservationFailure::Unacknowledged(
            AgentError::AuditFailure
        ))
    );

    release.send(()).unwrap();
    completion.await.unwrap();
    assert_eq!(audit.attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn stalled_first_close_cancellation_does_not_suppress_later_audit_or_receipt() {
    let (entered, waiting) = oneshot::channel();
    let audit = Arc::new(FirstSettlementStalls {
        entered: Mutex::new(Some(entered)),
        attempts: Mutex::new(Vec::new()),
        stalls: 1,
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let first = agent
        .enqueue(request("close-first"), Submitter::owner(actor()))
        .await
        .unwrap();
    let second = agent
        .enqueue(request("close-second"), Submitter::owner(actor()))
        .await
        .unwrap();
    let close = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(actor()).await }
    });
    waiting.await.unwrap();
    drop(_invocation);
    tokio::time::advance(AUDIT_OBSERVATION_TIMEOUT).await;
    assert!(close.await.unwrap().is_err());
    assert_eq!(audit.attempts.lock().unwrap().len(), 2);
    assert!(first.wait().await.is_err());
    assert_eq!(second.wait().await, Err(AgentError::Closed));
}

#[tokio::test(start_paused = true)]
async fn stalled_audit_observers_release_their_payloads_after_each_timeout() {
    let live = Arc::new(AtomicUsize::new(0));
    let audit = Arc::new(DroppingAudit {
        live: live.clone(),
        attempts: AtomicUsize::new(0),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    for _ in 0..4 {
        let observer = tokio::spawn({
            let agent = agent.clone();
            async move {
                agent
                    .record_scheduling_audit(test_admission(&agent), "retention_test")
                    .await
            }
        });
        while live.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(AUDIT_OBSERVATION_TIMEOUT).await;
        assert_eq!(
            observer.await.unwrap(),
            Err(AuditObservationFailure::Unacknowledged(
                AgentError::AuditFailure
            ))
        );
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }
    assert_eq!(audit.attempts.load(Ordering::SeqCst), 4);
}

#[tokio::test(start_paused = true)]
async fn each_bulk_settlement_gets_its_full_audit_observation_bound() {
    let (entered, waiting) = oneshot::channel();
    let audit = Arc::new(FirstSettlementStalls {
        entered: Mutex::new(Some(entered)),
        attempts: Mutex::new(Vec::new()),
        stalls: 2,
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let first = agent
        .enqueue(request("full-bound-first"), Submitter::owner(actor()))
        .await
        .unwrap();
    let second = agent
        .enqueue(request("full-bound-second"), Submitter::owner(actor()))
        .await
        .unwrap();
    let settlement = tokio::spawn({
        let agent = agent.clone();
        async move {
            let mut scheduler = agent.inner.scheduler.lock().await;
            agent
                .settle_failed_pending(
                    &mut scheduler,
                    AgentError::Protocol("failed attachment".into()),
                )
                .await
        }
    });
    waiting.await.unwrap();
    tokio::time::advance(AUDIT_OBSERVATION_TIMEOUT).await;
    while audit.attempts.lock().unwrap().len() < 2 {
        tokio::task::yield_now().await;
    }
    assert!(!settlement.is_finished());
    tokio::time::advance(AUDIT_OBSERVATION_TIMEOUT - Duration::from_secs(1)).await;
    assert!(!settlement.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(settlement.await.unwrap().is_err());
    for receipt in [first, second] {
        assert!(
            matches!(receipt.wait().await, Err(AgentError::MultipleOperationFailures {
            subsequent_error, ..
        }) if *subsequent_error == AgentError::AuditFailure)
        );
    }
}

fn test_admission(agent: &Agent) -> ExecutionAuditRecord {
    ExecutionAuditRecord::QueueAdmitted(QueueAdmissionRecord::submitted(
        agent.inner.manager.id().clone(),
        ExecutionId::new("audit-observer").unwrap(),
        SubmissionMode::Queued,
        actor(),
    ))
}

#[tokio::test(start_paused = true)]
async fn dropped_admission_caller_does_not_lose_timeout_evidence() {
    let (entered, waiting) = oneshot::channel();
    let audit = Arc::new(PendingAdmissionAudit {
        entered: Mutex::new(Some(entered)),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let caller = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .enqueue(request("lost-caller"), Submitter::owner(actor()))
                .await
        }
    });
    waiting.await.unwrap();
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    tokio::time::advance(AUDIT_OBSERVATION_TIMEOUT).await;
    let recovered = agent
        .enqueue(request("lost-caller"), Submitter::owner(actor()))
        .await
        .unwrap();
    assert!(
        matches!(recovered.evidence(), AdmissionEvidence::Failed(failure)
        if failure.audit() == Some(&AgentError::AuditFailure))
    );
    assert_eq!(recovered.wait().await, Err(AgentError::Closed));
}

#[tokio::test(start_paused = true)]
async fn audit_acknowledgement_wins_when_deadline_is_also_ready() {
    let (entered, waiting) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let clock = Arc::new(AuditClock::new());
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()), clock.clone())
        .await
        .unwrap();
    let agent = Agent::prepare(
        Arc::new(TestProvider),
        manager,
        Arc::new(GatedAudit {
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(Some(released)),
        }),
    )
    .await
    .unwrap();
    let observer = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .record_scheduling_audit(test_admission(&agent), "simultaneous_test")
                .await
        }
    });
    waiting.await.unwrap();
    // Neither wake yields the test task, so both branches are ready next poll.
    release.send(()).unwrap();
    clock.advance(AUDIT_OBSERVATION_TIMEOUT);
    assert_eq!(observer.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn scheduling_audit_panics_are_typed_and_observer_is_dropped() {
    for point in [AuditPanic::Creation, AuditPanic::Poll, AuditPanic::Drop] {
        let agent = prepared(Arc::new(InMemoryStorage::new()), Arc::new(point)).await;
        assert_eq!(
            agent
                .record_scheduling_audit(test_admission(&agent), "panic_test")
                .await,
            Err(AuditObservationFailure::Panicked)
        );
    }
}

#[tokio::test]
async fn scheduling_audit_clock_panic_remains_distinct_from_a_sink_failure() {
    #[derive(Clone, Copy)]
    enum ClockFault {
        Read,
        Construction,
        Poll,
        Drop,
    }
    struct PanickingClock(ClockFault);
    struct FaultingWake(ClockFault);
    impl Future for FaultingWake {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            assert!(!matches!(self.0, ClockFault::Poll), "audit clock poll");
            Poll::Ready(())
        }
    }
    impl Drop for FaultingWake {
        fn drop(&mut self) {
            assert!(!matches!(self.0, ClockFault::Drop), "audit clock drop");
        }
    }
    impl MessageCommitClock for PanickingClock {
        fn now(&self) -> Duration {
            assert!(!matches!(self.0, ClockFault::Read), "audit clock read");
            Duration::ZERO
        }
        fn sleep_until(&self, _: Duration) -> MessageCommitSleep {
            assert!(
                !matches!(self.0, ClockFault::Construction),
                "audit clock construction"
            );
            Box::pin(FaultingWake(self.0))
        }
        fn unix_milliseconds(&self) -> u64 {
            0
        }
    }
    for point in [
        ClockFault::Read,
        ClockFault::Construction,
        ClockFault::Poll,
        ClockFault::Drop,
    ] {
        let manager = SessionManager::open(
            None,
            Arc::new(InMemoryStorage::new()),
            Arc::new(PanickingClock(point)),
        )
        .await
        .unwrap();
        let agent = Agent::prepare(
            Arc::new(TestProvider),
            manager,
            Arc::new(PendingAdmissionAudit {
                entered: Mutex::new(None),
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            agent
                .record_scheduling_audit(test_admission(&agent), "clock_panic_test")
                .await,
            Err(AuditObservationFailure::Panicked)
        );
    }
}

fn request(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new("queued input").unwrap()),
        estimated_input_tokens: 2,
        reserved_output_tokens: 10,
    }
}

fn actor() -> ActionContext {
    ActionContext::new("caller", "test", "failed-pending").unwrap()
}

#[tokio::test]
async fn failed_attachment_settlement_panics_do_not_discard_the_queue_tail() {
    let mutation = Arc::new(Mutex::new(None));
    let storage = Arc::new(PanickingQueueStorage {
        inner: InMemoryStorage::new(),
        mutation: mutation.clone(),
    });
    let audit = Arc::new(SettlementPanickingAudit::default());
    let agent = prepared(storage, audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let first_id = ExecutionId::new("failed-attachment-first").unwrap();
    let first = agent
        .enqueue(request(first_id.as_str()), Submitter::owner(actor()))
        .await
        .unwrap();
    let second = agent
        .enqueue(
            request("failed-attachment-second"),
            Submitter::owner(actor()),
        )
        .await
        .unwrap();
    mutation.lock().unwrap().replace(QueueMutation::Removed {
        id: first_id,
        cause: QueueRemovalCause::DispatchFailed,
    });
    let failure = AgentError::Protocol("automatic attachment failed".into());
    let mut scheduler = agent.inner.scheduler.lock().await;
    let aggregate = agent
        .settle_failed_pending(&mut scheduler, failure.clone())
        .await
        .unwrap_err();
    drop(scheduler);

    let first = first.wait().await;
    let second = second.wait().await;
    assert!(format!("{first:?}").contains("automatic attachment failed"));
    assert!(format!("{first:?}").contains("AuditFailure"));
    assert!(format!("{first:?}").contains("persistence panicked"));
    assert_eq!(second, Err(failure));
    assert!(format!("{aggregate:?}").contains("AuditFailure"));
    assert!(format!("{aggregate:?}").contains("persistence panicked"));
    assert_eq!(audit.settlements.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn automatic_failure_claim_precedes_close_while_settlement_audit_waits() {
    let (entered, waiting) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(Some((entered, released))),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let queued = agent
        .enqueue(request("failure-before-close"), Submitter::owner(actor()))
        .await
        .unwrap();
    let failure = AgentError::Protocol("automatic attachment failed".into());
    let settlement = tokio::spawn({
        let agent = agent.clone();
        let failure = failure.clone();
        async move {
            let mut scheduler = agent.inner.scheduler.lock().await;
            agent.settle_failed_pending(&mut scheduler, failure).await
        }
    });
    waiting.await.unwrap();
    let closer = ActionContext::new("closer", "test", "close-after-claim").unwrap();
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(closer));
    release.send(()).unwrap();

    assert_eq!(settlement.await.unwrap(), Ok(()));
    assert_eq!(queued.wait().await, Err(failure));
    {
        let records = audit.settlements.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].cause(), SchedulingCause::DispatchFailed);
        assert_eq!(records[0].initiator(), SchedulingInitiator::Automatic);
        assert_eq!(records[0].submitted_by(), &actor());
        assert_eq!(records[0].initiated_by(), None);
    }
    agent.inner.lifecycle.complete_stop(&attempt).await;
}

#[tokio::test]
async fn explicit_close_precedes_unclaimed_automatic_failure_settlement() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let _invocation = agent.inner.invocation.lock().await;
    let submitted_by = actor();
    let queued = agent
        .enqueue(
            request("close-before-failure"),
            Submitter::owner(submitted_by.clone()),
        )
        .await
        .unwrap();
    let closer = ActionContext::new("closer", "test", "close-before-claim").unwrap();
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(closer.clone()));
    let mut scheduler = agent.inner.scheduler.lock().await;
    assert_eq!(
        agent
            .settle_failed_pending(
                &mut scheduler,
                AgentError::Protocol("later automatic attachment failure".into()),
            )
            .await,
        Ok(())
    );
    drop(scheduler);

    assert_eq!(queued.wait().await, Err(AgentError::Closed));
    {
        let records = audit.settlements.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].cause(), SchedulingCause::SessionClosed);
        assert_eq!(records[0].initiator(), SchedulingInitiator::Caller);
        assert_eq!(records[0].submitted_by(), &submitted_by);
        assert_eq!(records[0].initiated_by(), Some(&closer));
    }
    agent.inner.lifecycle.complete_stop(&attempt).await;
}

// The runner tests below drive `run_queue` on the test's own task rather than
// spawning it. One poll runs it to its first wait, and the test asserts that
// poll was `Pending`, so "the runner is waiting" is checked, not assumed from
// the order in which the runtime polls tasks (which Tokio does not promise).
// Setting `running` first, as `start_runner` would, keeps admission from
// spawning a second runner. Hand-over of the invocation slot relies on Tokio's
// `Mutex` granting waiters in the order they called `lock`, which it documents.
async fn poll_once<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
    std::future::poll_fn(|context| Poll::Ready(Pin::new(&mut *future).poll(context))).await
}

// Row one of the orderings table in docs/agent_execution/scheduling.md. An
// attachment starts the runner whether or not anything is queued. With the
// scheduler lock held here, the runner stops at its first wait. A runner that
// took the invocation slot before finding the queue empty turned this direct
// invoke into Busy, which is how `dispatch_save_panic_does_not_inherit_previous_close_actor`
// hung (#366). Unattached, the invoke must instead reach the lifecycle's own
// refusal.
#[tokio::test]
async fn an_idle_queue_runner_leaves_the_invocation_slot_to_a_direct_invoke() {
    // No queued input reaches settlement here, so this audit never panics.
    let audit = Arc::new(SettlementPanickingAudit::default());
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let bound = Duration::from_secs(5);
    let mut scheduler = agent.inner.scheduler.lock().await;
    scheduler.running = true;
    let mut runner = Box::pin(agent.run_queue());
    assert!(poll_once(&mut runner).await.is_pending());

    let invoked = tokio::time::timeout(
        bound,
        agent.invoke(
            request("direct-beside-idle-runner"),
            Submitter::owner(actor()),
        ),
    )
    .await
    .expect("direct invoke stalled beside an idle runner");
    assert!(
        matches!(invoked, Err(AgentError::AttachmentUnavailable(_))),
        "{invoked:?}"
    );

    drop(scheduler);
    tokio::time::timeout(bound, runner)
        .await
        .expect("idle runner did not exit");
    assert!(!agent.inner.scheduler.lock().await.running);
    assert!(agent.inner.invocation.try_lock().is_ok());
}

// Row three. While a direct invocation holds the slot, so that no runner can
// finish, an admission while no runner is running starts one.
#[tokio::test]
async fn an_admission_while_no_runner_is_running_starts_one() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let _direct = agent.inner.invocation.clone().lock_owned().await;
    assert!(!agent.inner.scheduler.lock().await.running);
    tokio::time::timeout(
        Duration::from_secs(5),
        agent.enqueue(
            request("admitted-with-no-runner"),
            Submitter::owner(actor()),
        ),
    )
    .await
    .expect("admission stalled")
    .unwrap();
    assert!(
        agent.inner.scheduler.lock().await.running,
        "an admission while no runner was running did not start one"
    );
}

// Row six by removal, and the waiting half of row two (its selection is the
// integration test's). While a direct invocation holds the slot, the runner
// passes the pre-check and waits for it. Removing the input empties the queue
// under it. When the slot is released the runner owns it until it is polled,
// so a direct `invoke` in that window is Busy; once the runner has seen the
// empty queue it exits and releases the slot.
#[tokio::test]
async fn a_runner_whose_queue_empties_while_it_waits_releases_the_slot_and_stops() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let bound = Duration::from_secs(5);
    let direct = agent.inner.invocation.clone().lock_owned().await;
    agent.inner.scheduler.lock().await.running = true;
    let removed_id = ExecutionId::new("removed-while-the-runner-waits").unwrap();
    let removed = tokio::time::timeout(
        bound,
        agent.enqueue(request(removed_id.as_str()), Submitter::owner(actor())),
    )
    .await
    .expect("admission stalled")
    .unwrap();
    let mut runner = Box::pin(agent.run_queue());
    assert!(
        poll_once(&mut runner).await.is_pending(),
        "the runner did not wait for the slot"
    );

    assert_eq!(
        tokio::time::timeout(bound, agent.remove_queued(removed_id, actor()))
            .await
            .expect("removal stalled"),
        Ok(QueueRemoval::Removed)
    );
    assert_eq!(
        tokio::time::timeout(bound, removed.wait())
            .await
            .expect("the removed receipt did not settle"),
        Err(AgentError::Closed)
    );

    drop(direct);
    assert_eq!(
        agent
            .invoke(
                request("direct-before-the-runner-exits"),
                Submitter::owner(actor())
            )
            .await,
        Err(AgentError::Busy),
        "the released slot went to the waiting runner"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the runner did not exit after its queue emptied");
    assert!(!agent.inner.scheduler.lock().await.running);
    assert!(matches!(
        agent
            .invoke(
                request("direct-after-the-runner-exits"),
                Submitter::owner(actor())
            )
            .await,
        Err(AgentError::AttachmentUnavailable(_))
    ));
}

// Explicit close stops every permit, then drains the queue before it settles
// each owner (`close_scheduled`, `cancel_pending`). A panic that escapes that
// settlement leaves stopped owners in `pending` with no queue entry, and a
// later attachment starts a runner. Strand one by hand the same way. The first
// runner passed the pre-check while the entry was queued, so it waits for the
// slot and then exits past the owner (row six). A runner started with only the
// unstopped owner still passes the pre-check and takes the slot, then passes
// over it and exits (row four). Once it is stopped, a new runner must take the
// slot and settle it (row five), not exit because the queue is empty.
#[tokio::test]
async fn a_runner_settles_a_stopped_owner_that_is_no_longer_queued() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let bound = Duration::from_secs(5);
    let direct = agent.inner.invocation.clone().lock_owned().await;
    agent.inner.scheduler.lock().await.running = true;
    let stranded_id = ExecutionId::new("owner-without-a-queue-entry").unwrap();
    let stranded = tokio::time::timeout(
        bound,
        agent.enqueue(request(stranded_id.as_str()), Submitter::owner(actor())),
    )
    .await
    .expect("admission stalled")
    .unwrap();
    let mut runner = Box::pin(agent.run_queue());
    assert!(poll_once(&mut runner).await.is_pending());
    assert!(agent
        .inner
        .scheduler
        .lock()
        .await
        .queue
        .remove(&stranded_id)
        .is_some());
    drop(direct);
    assert_eq!(
        agent
            .invoke(
                request("direct-beside-the-stranded-owner"),
                Submitter::owner(actor())
            )
            .await,
        Err(AgentError::Busy),
        "the released slot went to the runner the stranded owner keeps"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the first runner did not exit");
    assert!(agent
        .inner
        .scheduler
        .lock()
        .await
        .pending
        .contains_key(&stranded_id));

    // Row four: a runner that starts with only that unstopped owner passes the
    // pre-check and waits for the slot, then takes it, passes over the owner
    // and exits, leaving it pending.
    let direct = agent.inner.invocation.clone().lock_owned().await;
    agent.inner.scheduler.lock().await.running = true;
    let mut runner = Box::pin(agent.run_queue());
    assert!(
        poll_once(&mut runner).await.is_pending(),
        "a runner with only an unstopped owner did not wait for the slot"
    );
    drop(direct);
    assert_eq!(
        agent
            .invoke(
                request("direct-beside-an-unstopped-owner"),
                Submitter::owner(actor())
            )
            .await,
        Err(AgentError::Busy),
        "a runner with only an unstopped owner did not take the slot"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the runner did not exit past the unstopped owner");
    assert!(agent
        .inner
        .scheduler
        .lock()
        .await
        .pending
        .contains_key(&stranded_id));

    // Row five: once stopped, a new runner takes the slot and settles it.
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(actor()));
    let direct = agent.inner.invocation.clone().lock_owned().await;
    {
        let mut scheduler = agent.inner.scheduler.lock().await;
        assert!(scheduler.pending[&stranded_id]
            ._work
            .cancellation()
            .is_some());
        scheduler.running = true;
    }
    let mut runner = Box::pin(agent.run_queue());
    assert!(
        poll_once(&mut runner).await.is_pending(),
        "a runner with a stopped owner to settle did not wait for the slot"
    );
    drop(direct);
    assert_eq!(
        agent
            .invoke(
                request("direct-beside-a-stopped-owner"),
                Submitter::owner(actor())
            )
            .await,
        Err(AgentError::Busy),
        "a runner with a stopped owner to settle did not take the slot"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the runner did not exit after settling the stopped owner");
    assert!(tokio::time::timeout(bound, stranded.wait())
        .await
        .expect("the stopped owner was never settled")
        .is_err());
    assert!(!agent
        .inner
        .scheduler
        .lock()
        .await
        .pending
        .contains_key(&stranded_id));
    agent.inner.lifecycle.complete_stop(&attempt).await;
}

// Row six, by close. While a direct invocation holds the slot, the runner
// passes the pre-check and waits for it. Close then settles the queued input
// and waits for the slot behind the runner. When the slot is released the
// runner owns it first, so a direct `invoke` is Busy until the runner has found
// the lifecycle closed and exited. Close then takes the slot and completes.
#[tokio::test]
async fn close_while_a_runner_waits_leaves_it_nothing_to_run_and_both_finish() {
    let audit = Arc::new(PausingSettlementAudit {
        settlements: Mutex::new(Vec::new()),
        gate: Mutex::new(None),
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit).await;
    let bound = Duration::from_secs(5);
    let direct = agent.inner.invocation.clone().lock_owned().await;
    agent.inner.scheduler.lock().await.running = true;
    let closed = tokio::time::timeout(
        bound,
        agent.enqueue(
            request("closed-while-the-runner-waits"),
            Submitter::owner(actor()),
        ),
    )
    .await
    .expect("admission stalled")
    .unwrap();
    let mut runner = Box::pin(agent.run_queue());
    assert!(
        poll_once(&mut runner).await.is_pending(),
        "the runner did not wait for the slot"
    );

    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(actor()).await }
    });
    assert_eq!(
        tokio::time::timeout(bound, closed.wait())
            .await
            .expect("close did not settle the queued receipt"),
        Err(AgentError::Closed)
    );
    assert!(agent.inner.scheduler.lock().await.queue.is_empty());

    drop(direct);
    assert_eq!(
        agent
            .invoke(
                request("direct-while-the-runner-holds-the-slot"),
                Submitter::owner(actor())
            )
            .await,
        Err(AgentError::Busy),
        "the released slot went to the waiting runner"
    );
    tokio::time::timeout(bound, runner)
        .await
        .expect("the runner did not exit after close emptied its queue");
    assert!(
        !agent.inner.scheduler.lock().await.running,
        "the runner exited past the closed lifecycle without clearing running"
    );
    let outcome = tokio::time::timeout(bound, closing)
        .await
        .expect("close stalled behind the runner")
        .expect("close task panicked");
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[tokio::test]
async fn report_modes_keep_final_and_suspend_pending_work_semantics() {
    for suspend in [false, true] {
        let agent = prepared(
            Arc::new(InMemoryStorage::new()),
            Arc::new(SettlementPanickingAudit {
                panic_once: AtomicBool::new(true),
                ..Default::default()
            }),
        )
        .await;
        let invocation = agent.inner.invocation.lock().await;
        let receipt = agent
            .enqueue(request("report-pending"), Submitter::owner(actor()))
            .await
            .unwrap();
        let closing = tokio::spawn({
            let agent = agent.clone();
            async move {
                if suspend {
                    agent.suspend_report(actor()).await
                } else {
                    agent.close_report(actor()).await
                }
            }
        });
        assert_eq!(receipt.wait().await, Err(AgentError::Closed));
        drop(invocation);
        let report = closing.await.unwrap().unwrap();
        assert!(report.cleanup().is_confirmed());
        assert_eq!(report.scheduling(), &Ok(()));
        let history = agent.session_manager().snapshot().await.unwrap();
        assert_eq!(
            history.invocations[0].scheduling.last().unwrap().cause,
            if suspend {
                SchedulingCause::Suspended
            } else {
                SchedulingCause::SessionClosed
            }
        );
        assert_eq!(agent.owed_turns().await.len(), usize::from(suspend));
    }
}

#[tokio::test(start_paused = true)]
async fn report_retains_failed_pending_cancellation_separately_from_cleanup() {
    let (entered, waiting) = oneshot::channel();
    let audit = Arc::new(FirstSettlementStalls {
        entered: Mutex::new(Some(entered)),
        attempts: Mutex::new(Vec::new()),
        stalls: 1,
    });
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let invocation = agent.inner.invocation.lock().await;
    let first = agent
        .enqueue(request("report-first"), Submitter::owner(actor()))
        .await
        .unwrap();
    let second = agent
        .enqueue(request("report-second"), Submitter::owner(actor()))
        .await
        .unwrap();
    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close_report(actor()).await }
    });
    waiting.await.unwrap();
    drop(invocation);
    tokio::time::advance(AUDIT_OBSERVATION_TIMEOUT).await;
    let report = closing.await.unwrap().unwrap();
    assert!(report.cleanup().is_confirmed());
    assert!(report.scheduling().is_err());
    let derived = report.clone().with_cleanup(report.cleanup().clone());
    assert_eq!(derived.scheduling(), report.scheduling());
    assert!(derived.into_result().is_err());
    assert!(report.into_result().is_err());
    assert_eq!(audit.attempts.lock().unwrap().len(), 2);
    assert!(first.wait().await.is_err());
    assert_eq!(second.wait().await, Err(AgentError::Closed));
}

#[tokio::test]
async fn close_report_bounds_bulk_cancellation_debt_without_releasing_its_facts() {
    struct FailedCancellations(AtomicUsize);
    impl ExecutionAudit for FailedCancellations {
        fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
            Box::pin(async move {
                if matches!(record, ExecutionAuditRecord::QueueSettled(_)) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                    Err(AgentError::AuditFailure)
                } else {
                    Ok(())
                }
            })
        }
    }
    let count = super::super::error_limits::DiagnosticTreeLimits::DEPTH + 2;
    let audit = Arc::new(FailedCancellations(AtomicUsize::new(0)));
    let agent = prepared(Arc::new(InMemoryStorage::new()), audit.clone()).await;
    let invocation = agent.inner.invocation.lock().await;
    let mut receipts = Vec::new();
    for n in 0..count {
        receipts.push(
            agent
                .enqueue(
                    request(&format!("bounded-cancellation-{n}")),
                    Submitter::owner(actor()),
                )
                .await
                .unwrap(),
        );
    }
    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close_report(actor()).await }
    });
    for receipt in receipts {
        assert!(
            matches!(receipt.wait().await, Err(AgentError::MultipleOperationFailures { first_error, subsequent_error }) if *first_error == AgentError::Closed && *subsequent_error == AgentError::AuditFailure)
        );
    }
    drop(invocation);
    let report = closing.await.unwrap().unwrap();
    assert!(report.cleanup().is_confirmed());
    assert_eq!(audit.0.load(Ordering::SeqCst), count);
    assert!(report
        .scheduling()
        .as_ref()
        .unwrap_err()
        .validate_retained_size()
        .is_ok());
    assert!(report.into_result().is_err());
}
