//! Constructor cancellation and cleanup retain the same exclusive storage lease.
mod cleanup_panics;
mod open_panics;
mod save_panics;
use super::*;
use std::sync::atomic::AtomicBool;
use tokio::{sync::Notify, time::advance};

struct CleanupProbe {
    started: AtomicBool,
    panic: Mutex<Option<cleanup_panics::Failure>>,
    result: Mutex<CleanupReport>,
    attempts: AtomicUsize,
    entered: Notify,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
    reasons: Mutex<Vec<SessionCloseRequest>>,
    execution_result: Mutex<ProviderExecutionReply>,
}
impl CleanupProbe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: AtomicBool::new(false),
            panic: Mutex::new(None),
            result: Mutex::new(CleanupReport::unconfirmed(AgentError::CleanupUncertain)),
            attempts: AtomicUsize::new(0),
            entered: Notify::new(),
            gate: Mutex::new(None),
            reasons: Mutex::new(Vec::new()),
            execution_result: Mutex::new(ProviderExecutionReply::Rejected(
                AgentError::Unsupported("initialization test".into()),
            )),
        })
    }
    async fn cleanup(&self, request: SessionCloseRequest) -> CleanupReport {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.reasons.lock().unwrap().push(request);
        let gate = self.gate.lock().unwrap().take();
        self.entered.notify_one();
        if let Some(gate) = gate {
            gate.await.unwrap();
        }
        self.result.lock().unwrap().clone()
    }
}
impl ProviderCleanup for CleanupProbe {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        self.cleanup_future(SessionCloseRequest::SessionFailed)
    }
}
impl ProviderSessionBackend for CleanupProbe {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            self.started.store(true, Ordering::SeqCst);
            self.execution_result.lock().unwrap().clone()
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        self.cleanup_future(request)
    }
}
struct OpeningProbe {
    cleanup: Arc<CleanupProbe>,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
    entered: Notify,
    fail_open: AtomicBool,
    events: Mutex<Option<Box<dyn ExecutionEventStream>>>,
}
impl OpeningProbe {
    fn new(cleanup: Arc<CleanupProbe>) -> Arc<Self> {
        Arc::new(Self {
            cleanup,
            gate: Mutex::new(None),
            entered: Notify::new(),
            fail_open: AtomicBool::new(false),
            events: Mutex::new(None),
        })
    }
}
impl AgentProvider for OpeningProbe {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("cleanup-probe", "none", "local").unwrap()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            let gate = self.gate.lock().unwrap().take();
            self.entered.notify_one();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            if self.fail_open.load(Ordering::SeqCst) {
                return Err(ProviderOpenError::with_cleanup(
                    AgentError::CleanupUncertain,
                    self.cleanup.clone(),
                ));
            }
            let (_, events) = mpsc::unbounded_channel();
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("returned-context").unwrap(),
                    self.cleanup.clone(),
                    capabilities(),
                    Arc::new(AcceptingAudit),
                ),
                events: self
                    .events
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap_or_else(|| Box::new(TestEvents(events))),
            })
        })
    }
}
async fn assert_busy(storage: &MemoryStorage) {
    assert!(matches!(
        SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(storage.clone())
        )
        .await,
        Err(StorageError::Busy)
    ));
}
async fn assert_released(storage: &MemoryStorage) {
    drop(storage.manager().await);
}

#[tokio::test]
async fn failed_initialization_retains_lease_until_explicit_cleanup_recovers() {
    for failure in ["fresh-save", "restored-save", "restore-identity", "open"] {
        let storage = MemoryStorage::default();
        let cleanup = CleanupProbe::new();
        let provider = OpeningProbe::new(cleanup.clone());
        if failure.starts_with("restore") {
            storage.0.lock().unwrap().snapshot = Some(SessionSnapshot {
                queue_history: Vec::new(),
                id: SessionId::new("conversation").unwrap(),
                provider: provider.identity(),
                provider_session_id: ExecutionSessionId::new(if failure == "restore-identity" {
                    "different-context"
                } else {
                    "returned-context"
                })
                .unwrap(),
                invocations: Vec::new(),
            });
        }
        if failure.ends_with("save") {
            storage.fail_next();
        }
        provider
            .fail_open
            .store(failure == "open", Ordering::SeqCst);
        let error = Agent::new(provider, storage.manager().await)
            .await
            .err()
            .unwrap();
        let original = error.cause().clone();
        assert!(error.needs_cleanup(), "{failure}");
        assert_busy(&storage).await;
        assert_eq!(
            error.retry_cleanup().await,
            Err(AgentError::CleanupUncertain)
        );
        assert_busy(&storage).await;
        *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
        error.retry_cleanup().await.unwrap();
        assert!(!error.needs_cleanup());
        assert_released(&storage).await;
        let attempts = cleanup.attempts.load(Ordering::SeqCst);
        error.retry_cleanup().await.unwrap();
        assert_eq!(cleanup.attempts.load(Ordering::SeqCst), attempts);
        assert_eq!(error.cause(), &original);
    }
}

#[tokio::test(start_paused = true)]
async fn dropping_agent_or_initialization_error_retries_cleanup_without_releasing_lease_early() {
    for initialization_error in [false, true] {
        let storage = MemoryStorage::default();
        let cleanup = CleanupProbe::new();
        let provider = OpeningProbe::new(cleanup.clone());
        if initialization_error {
            storage.fail_next();
        }
        let opened = Agent::new(provider, storage.manager().await).await;
        if initialization_error {
            cleanup.entered.notified().await;
        }
        drop(opened);
        cleanup.entered.notified().await;
        assert_busy(&storage).await;
        let attempts = cleanup.attempts.load(Ordering::SeqCst);
        advance(Duration::from_millis(99)).await;
        tokio::task::yield_now().await;
        assert_eq!(cleanup.attempts.load(Ordering::SeqCst), attempts);
        *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
        advance(Duration::from_millis(1)).await;
        cleanup.entered.notified().await;
        assert_released(&storage).await;
        assert_eq!(
            cleanup.reasons.lock().unwrap().last().unwrap(),
            &if initialization_error {
                SessionCloseRequest::SessionFailed
            } else {
                SessionCloseRequest::SessionHandlesDropped
            }
        );
    }
}

#[tokio::test(start_paused = true)]
async fn abandoned_constructor_retains_ownership_during_open_save_and_cleanup() {
    for phase in ["open", "failed-open", "save", "cleanup"] {
        let storage = MemoryStorage::default();
        let cleanup = CleanupProbe::new();
        let provider = OpeningProbe::new(cleanup.clone());
        let (release, wait) = oneshot::channel();
        let mut wait = Some(wait);
        let save_gate = if phase == "save" {
            Some(storage.pause_next_save())
        } else {
            None
        };
        provider
            .fail_open
            .store(phase == "failed-open", Ordering::SeqCst);
        if phase == "open" || phase == "failed-open" {
            *provider.gate.lock().unwrap() = wait.take();
        }
        if phase == "cleanup" {
            storage.fail_next();
            *cleanup.gate.lock().unwrap() = wait.take();
        }
        let manager = storage.manager().await;
        let caller = tokio::spawn({
            let provider = provider.clone();
            async move { Agent::new(provider, manager).await }
        });
        match phase {
            "open" | "failed-open" => provider.entered.notified().await,
            "save" => {}
            _ => cleanup.entered.notified().await,
        }
        let save_release = if let Some((started, release)) = save_gate {
            started.await.unwrap();
            Some(release)
        } else {
            None
        };
        caller.abort();
        assert!(caller.await.err().unwrap().is_cancelled());
        assert_busy(&storage).await;
        if let Some(save_release) = save_release {
            save_release.send(()).unwrap();
        } else {
            release.send(()).unwrap();
        }
        cleanup.entered.notified().await;
        assert_busy(&storage).await;
        *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
        advance(Duration::from_millis(100)).await;
        cleanup.entered.notified().await;
        assert_released(&storage).await;
    }
}

#[tokio::test]
async fn audit_only_initialization_cleanup_failure_does_not_keep_a_confirmed_lease() {
    let storage = MemoryStorage::default();
    storage.fail_next();
    let cleanup = CleanupProbe::new();
    *cleanup.result.lock().unwrap() = cleaned_with_error(AgentError::AuditFailure);
    let error = Agent::new(OpeningProbe::new(cleanup.clone()), storage.manager().await)
        .await
        .err()
        .unwrap();
    assert!(!error.needs_cleanup());
    assert!(
        matches!(error.cause(), AgentError::StorageInitialization { cleanup_result, .. } if **cleanup_result == Err(AgentError::AuditFailure))
    );
    assert_released(&storage).await;
    let attempts = cleanup.attempts.load(Ordering::SeqCst);
    for _ in 0..3 {
        assert_eq!(error.retry_cleanup().await, Err(AgentError::AuditFailure));
        assert!(!error.needs_cleanup());
        assert_eq!(cleanup.attempts.load(Ordering::SeqCst), attempts);
        assert_released(&storage).await;
    }
}

#[tokio::test(start_paused = true)]
async fn confirmed_close_releases_immediately_but_resume_rearms_drop_protection() {
    let storage = MemoryStorage::default();
    let cleanup = CleanupProbe::new();
    *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
    let provider = OpeningProbe::new(cleanup.clone());
    let agent = Agent::new(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    cleanup.entered.notified().await;
    drop(agent);
    assert_released(&storage).await;

    let agent = Agent::new(provider, storage.manager().await).await.unwrap();
    agent.close(close_action()).await.unwrap();
    cleanup.entered.notified().await;
    assert!(matches!(
        agent.invoke(request("restore"), close_action()).await,
        Err(AgentError::Unsupported(_))
    ));
    *cleanup.result.lock().unwrap() = CleanupReport::unconfirmed(AgentError::CleanupUncertain);
    drop(agent);
    cleanup.entered.notified().await;
    assert_busy(&storage).await;
    *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
    advance(Duration::from_millis(100)).await;
    cleanup.entered.notified().await;
    assert_released(&storage).await;
}

#[tokio::test]
async fn cleanup_retry_serializes_and_cancellation_preserves_recovery_ownership() {
    let storage = MemoryStorage::default();
    storage.fail_next();
    let cleanup = CleanupProbe::new();
    let error = Arc::new(
        Agent::new(OpeningProbe::new(cleanup.clone()), storage.manager().await)
            .await
            .err()
            .unwrap(),
    );
    cleanup.entered.notified().await;
    let (release, wait) = oneshot::channel();
    *cleanup.gate.lock().unwrap() = Some(wait);
    let first = tokio::spawn({
        let error = error.clone();
        async move { error.retry_cleanup().await }
    });
    cleanup.entered.notified().await;
    let attempts = cleanup.attempts.load(Ordering::SeqCst);
    let second = tokio::spawn({
        let error = error.clone();
        async move { error.retry_cleanup().await }
    });
    tokio::task::yield_now().await;
    assert_eq!(cleanup.attempts.load(Ordering::SeqCst), attempts);
    assert_busy(&storage).await;
    first.abort();
    assert!(first.await.err().unwrap().is_cancelled());
    drop(release);
    assert_eq!(second.await.unwrap(), Err(AgentError::CleanupUncertain));
    assert!(error.needs_cleanup());
    assert_busy(&storage).await;
    *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
    error.retry_cleanup().await.unwrap();
    assert_released(&storage).await;
}

#[tokio::test(start_paused = true)]
async fn uncertain_explicit_close_retains_its_actor_through_retry_and_final_drop() {
    let storage = MemoryStorage::default();
    let cleanup = CleanupProbe::new();
    let agent = Agent::new(OpeningProbe::new(cleanup.clone()), storage.manager().await)
        .await
        .unwrap();
    let actor = ActionContext::new("initiator", "surface", "first-close").unwrap();
    let request = SessionCloseRequest::Explicit(actor.clone());
    assert_eq!(agent.close(actor).await, Err(AgentError::CleanupUncertain));
    cleanup.entered.notified().await;
    assert_busy(&storage).await;
    assert_eq!(
        agent.close(close_action()).await,
        Err(AgentError::CleanupUncertain)
    );
    cleanup.entered.notified().await;
    drop(agent);
    cleanup.entered.notified().await;
    assert_busy(&storage).await;
    *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
    advance(Duration::from_millis(100)).await;
    cleanup.entered.notified().await;
    assert_released(&storage).await;
    assert_eq!(*cleanup.reasons.lock().unwrap(), vec![request; 4]);
}

struct FailureEvents(Option<ObservationFailure>, Arc<CleanupProbe>);
impl ExecutionEventStream for FailureEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async {
            // This fixture produces invocation observations only after execute starts.
            if !self.1.started.load(Ordering::SeqCst) {
                return std::future::pending().await;
            }
            self.0.take().map_or(Ok(None), Err)
        })
    }
}

#[tokio::test(start_paused = true)]
async fn uncertain_failure_cleanup_retains_its_cause_through_explicit_retry_and_drop() {
    for (failure, cause, close_request) in [
        (
            AgentError::Deadline,
            ObservationFailureCause::DeadlineExceeded,
            SessionCloseRequest::DeadlineExceeded,
        ),
        (
            AgentError::Transport("reader failed".into()),
            ObservationFailureCause::ExecutionFailed,
            SessionCloseRequest::ExecutionFailed,
        ),
        // Lifecycle cause belongs to the observation report, independently of
        // misleading or bounded provider diagnostics.
        (
            AgentError::Deadline,
            ObservationFailureCause::ExecutionFailed,
            SessionCloseRequest::ExecutionFailed,
        ),
        (
            AgentError::Transport("x".repeat(1024 * 1024)),
            ObservationFailureCause::DeadlineExceeded,
            SessionCloseRequest::DeadlineExceeded,
        ),
    ] {
        let storage = MemoryStorage::default();
        let cleanup = CleanupProbe::new();
        *cleanup.execution_result.lock().unwrap() =
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(Err(AgentError::Transport("execution failed".into()))),
                None,
                ProviderSessionState::Usable,
            ));
        let provider = OpeningProbe::new(cleanup.clone());
        *provider.events.lock().unwrap() = Some(Box::new(FailureEvents(
            Some(ObservationFailure::new(failure, cause)),
            cleanup.clone(),
        )));
        let agent = Agent::new(provider, storage.manager().await).await.unwrap();
        assert!(matches!(
            agent
                .invoke(request("failure-cleanup"), close_action())
                .await,
            Err(AgentError::ExecutionObservation { .. })
        ));
        cleanup.entered.notified().await;
        assert_eq!(
            agent.close(close_action()).await,
            Err(AgentError::CleanupUncertain)
        );
        cleanup.entered.notified().await;
        drop(agent);
        cleanup.entered.notified().await;
        assert_busy(&storage).await;
        *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
        advance(Duration::from_millis(100)).await;
        cleanup.entered.notified().await;
        assert_released(&storage).await;
        assert_eq!(*cleanup.reasons.lock().unwrap(), vec![close_request; 4]);
    }
}

#[tokio::test(start_paused = true)]
async fn confirmed_cleanup_allows_a_resumed_attachment_to_record_a_new_drop_cause() {
    let storage = MemoryStorage::default();
    let cleanup = CleanupProbe::new();
    let agent = Agent::new(OpeningProbe::new(cleanup.clone()), storage.manager().await)
        .await
        .unwrap();
    let first = SessionCloseRequest::Explicit(close_action());
    assert_eq!(
        agent.close(close_action()).await,
        Err(AgentError::CleanupUncertain)
    );
    cleanup.entered.notified().await;
    *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
    agent
        .close(ActionContext::new("other", "surface", "retry").unwrap())
        .await
        .unwrap();
    cleanup.entered.notified().await;
    assert!(matches!(
        agent.invoke(request("resume"), close_action()).await,
        Err(AgentError::Unsupported(_))
    ));
    drop(agent);
    cleanup.entered.notified().await;
    assert_released(&storage).await;
    assert_eq!(
        *cleanup.reasons.lock().unwrap(),
        vec![
            first.clone(),
            first,
            SessionCloseRequest::SessionHandlesDropped
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn abandoning_failure_cleanup_preserves_its_request_before_provider_settlement() {
    let storage = MemoryStorage::default();
    let cleanup = CleanupProbe::new();
    *cleanup.execution_result.lock().unwrap() =
        ProviderExecutionReply::Finished(ExecutionReport::new(
            Some(Err(AgentError::Transport("execution failed".into()))),
            None,
            ProviderSessionState::Usable,
        ));
    let provider = OpeningProbe::new(cleanup.clone());
    *provider.events.lock().unwrap() = Some(Box::new(FailureEvents(
        Some(ObservationFailure::new(
            AgentError::Deadline,
            ObservationFailureCause::DeadlineExceeded,
        )),
        cleanup.clone(),
    )));
    let agent = Agent::new(provider, storage.manager().await).await.unwrap();
    let (release, wait) = oneshot::channel();
    *cleanup.gate.lock().unwrap() = Some(wait);
    let invoking = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .invoke(request("abandoned-cleanup"), close_action())
                .await
        }
    });
    cleanup.entered.notified().await;
    invoking.abort();
    assert!(invoking.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    drop(agent);
    cleanup.entered.notified().await;
    assert_busy(&storage).await;
    *cleanup.result.lock().unwrap() = CleanupReport::confirmed(CloseOutcome { forced: false });
    advance(Duration::from_millis(100)).await;
    cleanup.entered.notified().await;
    assert_released(&storage).await;
    assert_eq!(
        *cleanup.reasons.lock().unwrap(),
        vec![SessionCloseRequest::DeadlineExceeded; 3]
    );
}

struct FailedOpening {
    cause: AgentError,
    cleanup: Arc<CleanupProbe>,
}
impl AgentProvider for FailedOpening {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("failed-opening", "fixture", "local").unwrap()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Err(ProviderOpenError::with_cleanup(
                self.cause.clone(),
                self.cleanup.clone(),
            ))
        })
    }
}

#[tokio::test]
async fn every_uncertain_opening_cause_retains_lease_on_fresh_and_restored_sessions() {
    for restored in [false, true] {
        for cause in [
            AgentError::CleanupUncertain,
            AgentError::AuditAndCleanupFailure,
            AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(AgentError::Deadline),
                cleanup_error: Box::new(AgentError::Transport("termination failed".into())),
            },
            AgentError::DiagnosticLimit,
        ] {
            let storage = MemoryStorage::default();
            let cleanup = CleanupProbe::new();
            let provider = Arc::new(FailedOpening {
                cause: cause.clone(),
                cleanup: cleanup.clone(),
            });
            if restored {
                storage.0.lock().unwrap().snapshot = Some(SessionSnapshot {
                    queue_history: Vec::new(),
                    id: SessionId::new("conversation").unwrap(),
                    provider: provider.identity(),
                    provider_session_id: ExecutionSessionId::new("restored-context").unwrap(),
                    invocations: Vec::new(),
                });
            }
            let error = Agent::new(provider, storage.manager().await)
                .await
                .err()
                .unwrap();
            assert_eq!(error.cause(), &cause);
            assert!(error.needs_cleanup());
            assert_busy(&storage).await;
            assert_eq!(
                error.retry_cleanup().await,
                Err(AgentError::CleanupUncertain)
            );
            assert_busy(&storage).await;
            // The explicit unconfirmed report retains ownership even when diagnostics
            // are opaque or nested; their shape supplies no cleanup authority.
            let contradiction = AgentError::DiagnosticLimit;
            for cleanup_error in [
                contradiction.clone(),
                AgentError::StorageDuringClose {
                    error: StorageError::Io("cleanup evidence failed".into()),
                    cleanup_result: Box::new(Err(contradiction)),
                },
            ] {
                *cleanup.result.lock().unwrap() = CleanupReport::unconfirmed(cleanup_error.clone());
                assert_eq!(error.retry_cleanup().await, Err(cleanup_error));
                assert!(error.needs_cleanup());
                assert_busy(&storage).await;
            }
            // Audit rejection must remain visible even when cleanup is confirmed.
            *cleanup.result.lock().unwrap() = cleaned_with_error(AgentError::AuditFailure);
            assert_eq!(error.retry_cleanup().await, Err(AgentError::AuditFailure));
            assert!(!error.needs_cleanup());
            assert_released(&storage).await;
            let attempts = cleanup.attempts.load(Ordering::SeqCst);
            for _ in 0..3 {
                assert_eq!(error.retry_cleanup().await, Err(AgentError::AuditFailure));
                assert_eq!(cleanup.attempts.load(Ordering::SeqCst), attempts);
                assert!(!error.needs_cleanup());
                assert_released(&storage).await;
            }
            assert_eq!(error.cause(), &cause);
        }
    }
}
