//! The warm-up runs once, off the request path, and leaves evidence behind.
use super::{AgentWarmUp, RuntimeFingerprint, WarmUpCause, WarmUpState};
use crate::agent_warm_up::application::{
    ProviderFailure, WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpFuture, WarmUpRecords,
};
use crate::conversation_test_support::{AcceptingAudit, Provider, ProviderFactory, TestClock};
use nessa_sdk::application::agent_execution::agents::{
    AgentError, AgentFuture, AgentStartupContext, AgentStartupPhase, AgentStartupStep,
};
use nessa_sdk::application::agent_execution::executions::{
    AttachmentAuditStage, ExecutionAudit, ExecutionAuditRecord,
};
use nessa_sdk::application::agent_execution::providers::{
    AgentProvider, CleanupFuture, CleanupReport, CloseOutcome, ProviderCleanup, ProviderIdentity,
    ProviderOpenError, ProviderOpenFuture, SessionCloseRequest,
};
use nessa_sdk::application::agent_execution::sessions::{
    ProviderContext, SessionSnapshot, SessionStorage, SessionStorageLease, StorageError,
    StorageFuture,
};
use nessa_sdk::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::{oneshot, Barrier};

fn runtime() -> RuntimeFingerprint {
    RuntimeFingerprint::new("gateway-test", "test", "sha256:aa").unwrap()
}

#[derive(Default)]
struct MemoryRecords {
    completed: Mutex<Vec<RuntimeFingerprint>>,
    reads: AtomicUsize,
    read_failure: Mutex<Option<String>>,
    read_panic: AtomicBool,
    write_failure: Mutex<Option<String>>,
}
impl WarmUpRecords for MemoryRecords {
    fn completed(&self, runtime: &RuntimeFingerprint) -> WarmUpFuture<'_, bool> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        assert!(
            !self.read_panic.load(Ordering::SeqCst),
            "record store panicked"
        );
        if let Some(failure) = self.read_failure.lock().unwrap().clone() {
            return Box::pin(async move { Err(WarmUpError::Records(failure)) });
        }
        let known = self.completed.lock().unwrap().contains(runtime);
        Box::pin(async move { Ok(known) })
    }
    fn record_completed(
        &self,
        runtime: RuntimeFingerprint,
        _observed_at_ms: u64,
    ) -> WarmUpFuture<'_, ()> {
        if let Some(failure) = self.write_failure.lock().unwrap().clone() {
            return Box::pin(async move { Err(WarmUpError::Records(failure)) });
        }
        self.completed.lock().unwrap().push(runtime);
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct RecordingAudit {
    records: Mutex<Vec<WarmUpAuditRecord>>,
    failure: Mutex<Option<String>>,
}
impl WarmUpAudit for RecordingAudit {
    fn record(&self, record: WarmUpAuditRecord) -> WarmUpFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        let failure = self.failure.lock().unwrap().clone();
        Box::pin(async move {
            match failure {
                Some(failure) => Err(WarmUpError::Audit(failure)),
                None => Ok(()),
            }
        })
    }
}

struct Fixture {
    warm_up: AgentWarmUp,
    provider: Arc<ProviderFactory>,
    records: Arc<MemoryRecords>,
    audit: Arc<RecordingAudit>,
}
fn fixture() -> Fixture {
    fixture_with(Arc::new(AcceptingAudit), Arc::new(InMemoryStorage::new()))
}
fn fixture_with(
    execution_audit: Arc<dyn ExecutionAudit>,
    storage: Arc<dyn SessionStorage>,
) -> Fixture {
    let provider = Arc::new(ProviderFactory::default());
    let records = Arc::new(MemoryRecords::default());
    let audit = Arc::new(RecordingAudit::default());
    Fixture {
        warm_up: AgentWarmUp::new(
            Arc::new(Provider::new(provider.clone())),
            execution_audit,
            storage,
            records.clone(),
            audit.clone(),
            Arc::new(TestClock),
            runtime(),
        ),
        provider,
        records,
        audit,
    }
}

struct RejectPublishedAudit;
impl ExecutionAudit for RejectPublishedAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if matches!(
                record,
                ExecutionAuditRecord::Attachment(record)
                    if record.after() == AttachmentAuditStage::ContextPublished
            ) {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}

struct FailPublicationStorage {
    inner: InMemoryStorage,
    failed: AtomicBool,
}

struct OpenFailureProvider {
    inner: Provider,
    cleanup: Arc<OpenFailureCleanup>,
}
struct OpenNoResourcesProvider(Provider);
struct OpenFailureCleanup {
    calls: AtomicUsize,
}
impl ProviderCleanup for OpenFailureCleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            CleanupReport::unconfirmed(AgentError::Transport("cleanup retained".into()))
        })
    }
}
impl AgentProvider for OpenFailureProvider {
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }
    fn open(
        &self,
        _restore: Option<nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId>,
    ) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            Err(ProviderOpenError::with_cleanup(
                AgentError::Protocol("provider open failed".into()),
                self.cleanup.clone(),
            ))
        })
    }
}
impl AgentProvider for OpenNoResourcesProvider {
    fn identity(&self) -> ProviderIdentity {
        self.0.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.0.capabilities()
    }
    fn open(
        &self,
        _restore: Option<nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId>,
    ) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Err(ProviderOpenError::no_resources(AgentError::Protocol(
                "provider open failed".into(),
            )))
        })
    }
}
struct FailPublicationLease {
    inner: Box<dyn SessionStorageLease>,
    failed: Arc<AtomicBool>,
}
impl SessionStorage for FailPublicationStorage {
    fn open(
        &self,
        id: nessa_sdk::domain::agent_execution::sessions::SessionId,
    ) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(FailPublicationLease {
                inner: self.inner.open(id).await?,
                failed: Arc::new(AtomicBool::new(self.failed.load(Ordering::SeqCst))),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for FailPublicationLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.inner.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        if matches!(snapshot.provider_context, ProviderContext::Recorded(_))
            && !self.failed.swap(true, Ordering::SeqCst)
        {
            return Box::pin(async { Err(StorageError::Io("publication rejected".into())) });
        }
        self.inner.save(snapshot)
    }
}

#[tokio::test]
async fn warming_opens_and_closes_one_real_session_and_records_it() {
    let fixture = fixture();
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.records.completed.lock().unwrap().as_slice(),
        [runtime()]
    );
    let records = fixture.audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.runtime, runtime());
    assert_eq!(record.before, WarmUpState::Cold);
    assert_eq!(record.after, WarmUpState::Warmed);
    assert!(record.failure.is_none());
    assert!(
        record.session_id.is_some(),
        "the session it opened is named"
    );
    assert!(!record.correlation_id.is_empty());
    assert!(record.observed_at_ms >= record.requested_at_ms);
    assert_eq!(record.cause, WarmUpCause::AutomaticPreparation);
    // The same context the provider session was closed with, so the SDK's own
    // closure evidence and this record cannot name different initiators — two
    // sets of constants drifting apart is exactly what that would look like.
    let closes = fixture.provider.close_requests.lock().unwrap();
    let [SessionCloseRequest::Explicit(closed_by)] = closes.as_slice() else {
        panic!("the warm-up closes its session explicitly")
    };
    assert_eq!(&record.initiator, closed_by);
    assert_eq!(record.initiator.principal_id(), "gateway");
    assert_eq!(record.initiator.surface_id(), "runtime_warm_up");
    assert_eq!(record.initiator.request_id(), record.correlation_id);
}

#[tokio::test]
async fn an_already_warmed_runtime_is_not_launched_again() {
    let fixture = fixture();
    fixture.records.completed.lock().unwrap().push(runtime());
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 0);
    // Nothing changed, so there is no transition to record.
    assert!(fixture.audit.records.lock().unwrap().is_empty());
}

/// The point of the whole exercise: a first message arriving while the runtime
/// is still starting waits for that launch rather than starting a second one.
#[tokio::test]
async fn a_caller_arriving_mid_warm_up_joins_it_instead_of_launching_again() {
    let fixture = fixture();
    let (release, gate) = oneshot::channel();
    *fixture.provider.open_gate.lock().unwrap() = Some(gate);
    let background = fixture.warm_up.clone();
    let started = Arc::new(Barrier::new(2));
    let waiting = tokio::spawn({
        let started = started.clone();
        async move {
            started.wait().await;
            background.wait_until_settled().await;
        }
    });
    fixture.warm_up.start();
    // The provider is inside `open` and blocked, so the warm-up is genuinely in
    // flight when the second caller arrives.
    fixture.provider.opening.notified().await;
    started.wait().await;
    let joining = tokio::spawn({
        let warm_up = fixture.warm_up.clone();
        async move { warm_up.wait_until_settled().await }
    });
    release.send(()).unwrap();
    waiting.await.unwrap();
    joining.await.unwrap();
    assert_eq!(
        fixture.provider.open_calls.load(Ordering::SeqCst),
        1,
        "one cold launch, however many callers waited on it"
    );
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_failed_audit_prevents_a_completion_record_and_leaves_the_runtime_cold() {
    let fixture = fixture();
    *fixture.audit.failure.lock().unwrap() = Some("sink rejected".into());
    fixture.warm_up.wait_until_settled().await;
    // The session was still opened and closed: audit failure must not prevent
    // the cleanup that had already happened.
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
    // Unrecorded evidence must not leave behind a record claiming success.
    assert!(fixture.records.completed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn confirmed_physical_cleanup_stays_distinct_from_attachment_audit_failure() {
    let fixture = fixture_with(
        Arc::new(RejectPublishedAudit),
        Arc::new(InMemoryStorage::new()),
    );
    fixture.warm_up.wait_until_settled().await;

    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert!(fixture.records.completed.lock().unwrap().is_empty());
    let records = fixture.audit.records.lock().unwrap();
    let failure = records[0].failure.as_ref().unwrap();
    assert_eq!(failure.error, AgentError::AuditFailure);
    assert!(!failure.cleanup_unconfirmed);
}

#[tokio::test]
async fn publication_failure_cleans_physical_resources_without_claiming_completion() {
    let fixture = fixture_with(
        Arc::new(AcceptingAudit),
        Arc::new(FailPublicationStorage {
            inner: InMemoryStorage::new(),
            failed: AtomicBool::new(false),
        }),
    );
    fixture.warm_up.wait_until_settled().await;

    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 1);
    assert!(fixture.records.completed.lock().unwrap().is_empty());
    let records = fixture.audit.records.lock().unwrap();
    let failure = records[0].failure.as_ref().unwrap();
    assert!(matches!(
        &failure.error,
        AgentError::StorageInitialization { .. }
    ));
    assert!(!failure.cleanup_unconfirmed);
}

#[tokio::test]
async fn provider_open_failure_reports_retained_cleanup_handle_ownership() {
    let provider = Arc::new(ProviderFactory::default());
    let cleanup = Arc::new(OpenFailureCleanup {
        calls: AtomicUsize::new(0),
    });
    let records = Arc::new(MemoryRecords::default());
    let audit = Arc::new(RecordingAudit::default());
    let warm_up = AgentWarmUp::new(
        Arc::new(OpenFailureProvider {
            inner: Provider::new(provider),
            cleanup: cleanup.clone(),
        }),
        Arc::new(AcceptingAudit),
        Arc::new(InMemoryStorage::new()),
        records.clone(),
        audit.clone(),
        Arc::new(TestClock),
        runtime(),
    );
    warm_up.wait_until_settled().await;

    assert_eq!(cleanup.calls.load(Ordering::SeqCst), 1);
    assert!(records.completed.lock().unwrap().is_empty());
    let audit = audit.records.lock().unwrap();
    let failure = audit[0].failure.as_ref().unwrap();
    assert_eq!(
        failure.error,
        AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Protocol("provider open failed".into())),
            cleanup_error: Box::new(AgentError::Transport("cleanup retained".into())),
        }
    );
    assert!(failure.cleanup_unconfirmed);
}

#[tokio::test]
async fn provider_open_failure_without_resources_reports_confirmed_absence() {
    let provider = Arc::new(ProviderFactory::default());
    let records = Arc::new(MemoryRecords::default());
    let audit = Arc::new(RecordingAudit::default());
    let warm_up = AgentWarmUp::new(
        Arc::new(OpenNoResourcesProvider(Provider::new(provider))),
        Arc::new(AcceptingAudit),
        Arc::new(InMemoryStorage::new()),
        records.clone(),
        audit.clone(),
        Arc::new(TestClock),
        runtime(),
    );
    warm_up.wait_until_settled().await;

    assert!(records.completed.lock().unwrap().is_empty());
    let audit = audit.records.lock().unwrap();
    let failure = audit[0].failure.as_ref().unwrap();
    assert_eq!(
        failure.error,
        AgentError::Protocol("provider open failed".into())
    );
    assert!(!failure.cleanup_unconfirmed);
}

#[tokio::test]
async fn publication_failures_report_physical_cleanup_at_capture_time() {
    for audit_failure in [false, true] {
        let execution_audit: Arc<dyn ExecutionAudit> = if audit_failure {
            Arc::new(RejectPublishedAudit)
        } else {
            Arc::new(AcceptingAudit)
        };
        let storage: Arc<dyn SessionStorage> = if audit_failure {
            Arc::new(InMemoryStorage::new())
        } else {
            Arc::new(FailPublicationStorage {
                inner: InMemoryStorage::new(),
                failed: AtomicBool::new(false),
            })
        };
        let fixture = fixture_with(execution_audit, storage);
        fixture.provider.close_reports.lock().unwrap().extend([
            CleanupReport::unconfirmed(AgentError::Transport("cleanup retained".into())),
            CleanupReport::confirmed(CloseOutcome { forced: false }),
        ]);
        fixture.warm_up.wait_until_settled().await;

        while fixture.provider.close_calls.load(Ordering::SeqCst) < 2 {
            let finished = fixture.provider.close_finished.notified();
            if fixture.provider.close_calls.load(Ordering::SeqCst) >= 2 {
                break;
            }
            finished.await;
        }
        assert_eq!(fixture.provider.close_calls.load(Ordering::SeqCst), 2);
        assert!(fixture.records.completed.lock().unwrap().is_empty());
        let audit = fixture.audit.records.lock().unwrap();
        let failure = audit[0].failure.as_ref().unwrap();
        if audit_failure {
            assert!(failure.cleanup_unconfirmed);
            assert_eq!(
                failure.error,
                AgentError::OperationAndCleanupFailure {
                    operation_error: Box::new(AgentError::AuditFailure),
                    cleanup_error: Box::new(AgentError::Transport("cleanup retained".into())),
                }
            );
        } else {
            assert!(!failure.cleanup_unconfirmed);
            let AgentError::StorageInitialization {
                error,
                cleanup_result,
            } = &failure.error
            else {
                panic!(
                    "publication storage failure must retain its original category: {:?}",
                    failure.error
                )
            };
            assert_eq!(error, &StorageError::Io("publication rejected".into()));
            assert_eq!(
                cleanup_result.as_ref(),
                &Err(AgentError::Transport("cleanup retained".into())),
                "the first cleanup failure remains historical evidence after retry confirms release"
            );
        }
    }
}

#[tokio::test]
async fn a_failed_launch_is_audited_as_still_cold_and_not_recorded_complete() {
    let fixture = fixture();
    // A startup deadline, because that is the failure this change is about: the
    // record has to say which step ran out of budget and whether saved context
    // was involved, not merely that something went wrong.
    let deadline = AgentError::StartupDeadline(AgentStartupStep::new(
        AgentStartupPhase::Session,
        AgentStartupContext::New,
    ));
    *fixture.provider.close_failure.lock().unwrap() = Some(deadline.clone());
    fixture.warm_up.wait_until_settled().await;
    let records = fixture.audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].before, WarmUpState::Cold);
    assert_eq!(records[0].after, WarmUpState::Cold, "nothing was warmed");
    assert_eq!(
        records[0].failure,
        Some(ProviderFailure {
            error: deadline,
            cleanup_unconfirmed: true,
        }),
        "physical resource ownership survives independently of the provider diagnostic"
    );
    assert!(fixture.records.completed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_unreadable_record_store_does_not_launch_or_claim_completion() {
    let fixture = fixture();
    *fixture.records.read_failure.lock().unwrap() = Some("unreadable".into());
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_completion_write_keeps_its_audited_evidence() {
    let fixture = fixture();
    *fixture.records.write_failure.lock().unwrap() = Some("read-only".into());
    fixture.warm_up.wait_until_settled().await;
    let records = fixture.audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    // The warm-up did happen; only the record of it could not be kept, so the
    // next start pays for the launch again rather than trusting a lost write.
    assert_eq!(records[0].after, WarmUpState::Warmed);
    assert!(fixture.records.completed.lock().unwrap().is_empty());
}

/// One run per process, whoever asks. A second `wait` after the first settled
/// must not launch anything, even though nothing was recorded as complete.
#[tokio::test]
async fn the_run_is_not_repeated_after_it_has_settled() {
    let fixture = fixture();
    *fixture.records.write_failure.lock().unwrap() = Some("read-only".into());
    fixture.warm_up.wait_until_settled().await;
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.records.reads.load(Ordering::SeqCst), 1);
}

/// A panic inside the run belongs to the run's own task.
///
/// The conversation that waits here was promised a wait, not an unwind from
/// work it did not start — and it must not be parked forever either, so the
/// run still settles and the waiter still returns.
#[tokio::test]
async fn a_panicking_run_settles_the_waiters_instead_of_unwinding_into_them() {
    let fixture = fixture();
    fixture.records.read_panic.store(true, Ordering::SeqCst);
    fixture.warm_up.wait_until_settled().await;
    // Nothing was warmed and nothing claimed it was.
    assert!(fixture.records.completed.lock().unwrap().is_empty());
    assert!(fixture.audit.records.lock().unwrap().is_empty());
    // And the run is settled, so a later waiter returns rather than starting
    // a second one on top of a panic.
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.records.reads.load(Ordering::SeqCst), 1);
}

/// Cancelling a wait abandons the wait, not the launch.
///
/// The conversation waiter is cancellable by design — the caller disconnects,
/// or teardown supersedes it — and tokio's `OnceCell` would hand the next
/// caller a fresh initializer, so two cold launches would race. The run owns
/// its own task instead.
#[tokio::test]
async fn cancelling_a_wait_neither_abandons_the_run_nor_starts_a_second() {
    let fixture = fixture();
    let (release, gate) = oneshot::channel();
    *fixture.provider.open_gate.lock().unwrap() = Some(gate);

    let abandoned = tokio::spawn({
        let warm_up = fixture.warm_up.clone();
        async move { warm_up.wait_until_settled().await }
    });
    fixture.provider.opening.notified().await;
    abandoned.abort();
    let _ = abandoned.await;

    // The launch the abandoned waiter started is still the only one.
    release.send(()).unwrap();
    fixture.warm_up.wait_until_settled().await;
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.audit.records.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.records.completed.lock().unwrap().as_slice(),
        [runtime()]
    );
}
