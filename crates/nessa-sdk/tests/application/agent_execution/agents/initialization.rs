//! Prepared construction and attachment ownership use separate observable phases.
use super::*;
use nessa_sdk::infrastructure::session_storage::LocalFileStorage;
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};
use tempfile::tempdir;
use tokio::sync::Notify;

#[derive(Default)]
struct AttachmentAuditProbe {
    records: Mutex<Vec<AttachmentAuditRecord>>,
    reject: AtomicBool,
    reject_after: Mutex<Option<AttachmentAuditStage>>,
    entered: Notify,
    completed: Notify,
    gate_after: Mutex<Option<AttachmentAuditStage>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}
impl ExecutionAudit for AttachmentAuditProbe {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            let attachment_after = match &record {
                ExecutionAuditRecord::Attachment(record) => Some(record.after()),
                _ => None,
            };
            if let ExecutionAuditRecord::Attachment(record) = &record {
                let gated = self
                    .gate_after
                    .lock()
                    .unwrap()
                    .is_none_or(|stage| stage == record.after());
                self.records.lock().unwrap().push(record.clone());
                if gated {
                    self.entered.notify_one();
                    let release = { self.release.lock().unwrap().take() };
                    if let Some(release) = release {
                        let _ = release.await;
                    }
                }
            }
            let reject = self.reject.load(Ordering::SeqCst)
                && match attachment_after {
                    Some(after) => self
                        .reject_after
                        .lock()
                        .unwrap()
                        .is_none_or(|stage| stage == after),
                    _ => self.reject_after.lock().unwrap().is_none(),
                };
            let result = if reject {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            };
            self.completed.notify_one();
            result
        })
    }
}

#[derive(Clone, Copy)]
enum AuditPanic {
    Construct,
    Poll,
    Drop,
}
struct PanickingAudit(AuditPanic);
struct QueuePanickingAudit(AuditPanic);
struct QueueSettlementRejectingAudit;
struct AutomaticCloseAudit {
    starting_entered: Notify,
    starting_release: Mutex<Option<oneshot::Receiver<()>>>,
    closed_entered: Notify,
    closed_release: Mutex<Option<oneshot::Receiver<()>>>,
}
struct PanickingAuditFuture(AuditPanic);
impl ExecutionAudit for PanickingAudit {
    fn record(&self, _record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        if matches!(self.0, AuditPanic::Construct) {
            panic!("audit construction panic");
        }
        Box::pin(PanickingAuditFuture(self.0))
    }
}
impl ExecutionAudit for QueuePanickingAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        if !matches!(record, ExecutionAuditRecord::QueueAdmitted(_)) {
            return Box::pin(async { Ok(()) });
        }
        if matches!(self.0, AuditPanic::Construct) {
            panic!("queue audit construction panic");
        }
        Box::pin(PanickingAuditFuture(self.0))
    }
}
impl ExecutionAudit for QueueSettlementRejectingAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if matches!(record, ExecutionAuditRecord::QueueSettled(_)) {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}
impl ExecutionAudit for AutomaticCloseAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            match record {
                ExecutionAuditRecord::Attachment(record)
                    if record.after() == AttachmentAuditStage::Starting =>
                {
                    self.starting_entered.notify_one();
                    let release = self.starting_release.lock().unwrap().take();
                    if let Some(release) = release {
                        let _ = release.await;
                    }
                    Ok(())
                }
                ExecutionAuditRecord::Attachment(record)
                    if record.cause() == AttachmentAuditCause::Closed =>
                {
                    self.closed_entered.notify_one();
                    let release = self.closed_release.lock().unwrap().take();
                    if let Some(release) = release {
                        let _ = release.await;
                    }
                    Err(AgentError::AuditFailure)
                }
                ExecutionAuditRecord::QueueAdmitted(_) => Err(AgentError::AuditFailure),
                _ => Ok(()),
            }
        })
    }
}

#[derive(Clone, Copy)]
enum OpenPanic {
    Construct,
    Poll,
    Drop,
}
struct PanickingProvider(OpenPanic);
struct NoResourcesProvider;
struct GatedFailedOpenProvider {
    cleanup: Arc<GatedFailedOpenCleanup>,
}
struct GatedFailedOpenCleanup {
    entered: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}
struct GatedOpenProvider {
    inner: Arc<TestProvider>,
    entered: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}
struct PanickingOpen(OpenPanic);
#[derive(Clone, Copy)]
enum CleanupPanic {
    Construct,
    Poll,
    Drop,
    DropAfterCompletionFailure,
}
#[derive(Clone, Copy)]
enum SavePanic {
    Construct,
    Poll,
    Drop,
}
#[derive(Clone)]
struct PanickingStorage {
    backing: MemoryStorage,
    failure: Arc<Mutex<Option<SavePanic>>>,
}
struct PanickingStorageLease {
    backing: Box<dyn SessionStorageLease>,
    failure: Arc<Mutex<Option<SavePanic>>>,
}
struct PanickingSaveFuture<'a> {
    backing: StorageFuture<'a, ()>,
    failure: SavePanic,
}
struct PanickingCleanupProvider {
    failure: CleanupPanic,
}
struct PanickingCleanupBackend(Mutex<Option<CleanupPanic>>);
struct PanickingCleanupFuture(CleanupPanic);
impl AgentProvider for PanickingProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("panic-open", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        if matches!(self.0, OpenPanic::Construct) {
            panic!("open construction panic");
        }
        Box::pin(PanickingOpen(self.0))
    }
}
impl AgentProvider for NoResourcesProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("no-resources", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) })
    }
}
impl ProviderCleanup for GatedFailedOpenCleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.entered.notify_one();
            let release = self.release.lock().unwrap().take();
            if let Some(release) = release {
                let _ = release.await;
            }
            CleanupReport::confirmed(CloseOutcome { forced: false })
        })
    }
}
impl AgentProvider for GatedFailedOpenProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("gated-failed-open", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            Err(ProviderOpenError::with_cleanup(
                AgentError::Closed,
                self.cleanup.clone(),
            ))
        })
    }
}
impl AgentProvider for GatedOpenProvider {
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        let release = self.release.lock().unwrap().take();
        Box::pin(async move {
            self.entered.notify_one();
            if let Some(release) = release {
                let _ = release.await;
            }
            self.inner.open(request).await
        })
    }
}
impl Future for PanickingOpen {
    type Output = Result<OpenedProviderSession, ProviderOpenError>;
    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.0, OpenPanic::Poll) {
            panic!("open poll panic");
        }
        Poll::Ready(Err(ProviderOpenError::no_resources(AgentError::Closed)))
    }
}
impl Drop for PanickingOpen {
    fn drop(&mut self) {
        if matches!(self.0, OpenPanic::Drop) {
            panic!("open drop panic");
        }
    }
}
impl AgentProvider for PanickingCleanupProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("panic-cleanup", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let (_, events) = mpsc::unbounded_channel();
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("panic-cleanup-context").unwrap(),
                    Arc::new(PanickingCleanupBackend(Mutex::new(Some(self.failure)))),
                    capabilities(),
                ),
                events: Box::new(TestEvents(events)),
            })
        })
    }
}
impl ProviderSessionBackend for PanickingCleanupBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async { ProviderExecutionReply::Rejected(AgentError::Closed) })
    }
    fn answer_permission(
        &self,
        _answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, _origin: SessionCloseRequest) -> CleanupFuture<'_> {
        let Some(failure) = self.0.lock().unwrap().take() else {
            return Box::pin(async { CleanupReport::confirmed(CloseOutcome { forced: false }) });
        };
        if matches!(failure, CleanupPanic::Construct) {
            panic!("cleanup construction panic");
        }
        Box::pin(PanickingCleanupFuture(failure))
    }
}
impl Future for PanickingCleanupFuture {
    type Output = CleanupReport;
    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.0, CleanupPanic::Poll) {
            panic!("cleanup poll panic");
        }
        let report = CleanupReport::confirmed(CloseOutcome { forced: false });
        Poll::Ready(
            if matches!(self.0, CleanupPanic::DropAfterCompletionFailure) {
                report.with_completion_failure(Some(AgentError::Transport(
                    "cleanup completion failed".into(),
                )))
            } else {
                report
            },
        )
    }
}
impl Drop for PanickingCleanupFuture {
    fn drop(&mut self) {
        if matches!(
            self.0,
            CleanupPanic::Drop | CleanupPanic::DropAfterCompletionFailure
        ) {
            panic!("cleanup drop panic");
        }
    }
}
impl SessionStorage for PanickingStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(PanickingStorageLease {
                backing: self.backing.open(id).await?,
                failure: self.failure.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for PanickingStorageLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        let Some(failure) = self.failure.lock().unwrap().take() else {
            return self.backing.save(snapshot);
        };
        if matches!(failure, SavePanic::Construct) {
            panic!("save construction panic");
        }
        Box::pin(PanickingSaveFuture {
            backing: self.backing.save(snapshot),
            failure,
        })
    }
}
impl Future for PanickingSaveFuture<'_> {
    type Output = Result<(), StorageError>;
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.failure, SavePanic::Poll) {
            panic!("save poll panic");
        }
        self.backing.as_mut().poll(context)
    }
}
impl Drop for PanickingSaveFuture<'_> {
    fn drop(&mut self) {
        if matches!(self.failure, SavePanic::Drop) {
            panic!("save drop panic");
        }
    }
}
impl Future for PanickingAuditFuture {
    type Output = Result<(), AgentError>;
    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.0, AuditPanic::Poll) {
            panic!("audit poll panic");
        }
        Poll::Ready(Ok(()))
    }
}
impl Drop for PanickingAuditFuture {
    fn drop(&mut self) {
        if matches!(self.0, AuditPanic::Drop) {
            panic!("audit drop panic");
        }
    }
}

async fn prepared(
    provider: Arc<dyn AgentProvider>,
    storage: &MemoryStorage,
    audit: Arc<dyn ExecutionAudit>,
) -> Agent {
    Agent::prepare(provider, storage.manager().await, audit)
        .await
        .unwrap()
}

#[tokio::test]
async fn prepare_is_durable_without_opening_provider() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = prepared(provider.clone(), &storage, Arc::new(AcceptingAudit)).await;
    assert!(provider.calls.opens.lock().unwrap().is_empty());
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Absent);
    assert!(matches!(
        storage.snapshot().provider_context,
        ProviderContext::Absent
    ));
}

#[tokio::test]
async fn queued_work_admitted_before_attachment_binds_to_the_published_provider() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = prepared(provider.clone(), &storage, Arc::new(AcceptingAudit)).await;
    let queued = agent
        .enqueue(request("queued-before-attach"), actor())
        .await
        .unwrap();
    assert!(provider.calls.opens.lock().unwrap().is_empty());

    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();

    assert_eq!(queued.wait().await, Ok(ExecutionOutcome::Completed));
    assert_eq!(provider.calls.opens.lock().unwrap().len(), 1);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn initial_open_failure_settles_every_owned_queue_receipt() {
    let storage = MemoryStorage::default();
    let agent = prepared(
        Arc::new(NoResourcesProvider),
        &storage,
        Arc::new(AcceptingAudit),
    )
    .await;
    let first = agent
        .enqueue(request("first-before-failure"), actor())
        .await
        .unwrap();
    let second = agent
        .enqueue(request("second-before-failure"), actor())
        .await
        .unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();

    assert_eq!(
        agent.start_attachment(authorization).unwrap().wait().await,
        Err(AgentError::Closed)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), first.wait())
            .await
            .expect("first receipt settled"),
        Err(AgentError::Closed)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), second.wait())
            .await
            .expect("second receipt settled"),
        Err(AgentError::Closed)
    );
    for result in [
        agent
            .enqueue(request("after-failed-attachment"), actor())
            .await,
        agent
            .enqueue_steering(request("steering-after-failed-attachment"), actor())
            .await,
    ] {
        assert!(matches!(
            result,
            Err(AgentError::AttachmentUnavailable(AttachmentPhase::Failed(
                _
            )))
        ));
    }
    let snapshot = storage.snapshot();
    for record in &snapshot.invocations {
        assert_eq!(record.result, Some(Err(AgentError::Closed)));
        assert_eq!(
            record.scheduling.last().unwrap().stage,
            InvocationStage::Settled
        );
    }
    assert!(snapshot.queue_history.iter().any(|record| matches!(
        &record.mutation,
        QueueMutation::Removed { id, .. } if id.as_str() == "first-before-failure"
    )));
    drop(agent);
    let restored = Agent::prepare(
        Arc::new(NoResourcesProvider),
        storage.manager().await,
        Arc::new(AcceptingAudit),
    )
    .await
    .unwrap();
    let retry = restored
        .enqueue(request("first-before-failure"), actor())
        .await
        .unwrap();
    assert_eq!(retry.wait().await, Err(AgentError::Closed));
}

#[tokio::test]
async fn initial_open_failure_settles_all_receipts_despite_audit_and_storage_failures() {
    let storage = MemoryStorage::default();
    let agent = prepared(
        Arc::new(NoResourcesProvider),
        &storage,
        Arc::new(QueueSettlementRejectingAudit),
    )
    .await;
    let first = agent
        .enqueue(request("failed-evidence-first"), actor())
        .await
        .unwrap();
    let second = agent
        .enqueue(request("failed-evidence-second"), actor())
        .await
        .unwrap();
    storage.fail_scheduling("failed-evidence-first", InvocationStage::Settled);
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();

    assert!(agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .is_err());
    for receipt in [first, second] {
        let error = tokio::time::timeout(Duration::from_secs(1), receipt.wait())
            .await
            .expect("every receipt settles")
            .unwrap_err();
        assert!(matches!(
            error,
            AgentError::MultipleOperationFailures { .. } | AgentError::StorageAfterExecution { .. }
        ));
    }
}

#[tokio::test]
async fn failed_attachment_refuses_new_work_while_cleanup_is_still_running() {
    let storage = MemoryStorage::default();
    let (release, waiting) = oneshot::channel();
    let cleanup = Arc::new(GatedFailedOpenCleanup {
        entered: Notify::new(),
        release: Mutex::new(Some(waiting)),
    });
    let agent = prepared(
        Arc::new(GatedFailedOpenProvider {
            cleanup: cleanup.clone(),
        }),
        &storage,
        Arc::new(AcceptingAudit),
    )
    .await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    cleanup.entered.notified().await;

    assert!(matches!(
        agent
            .enqueue(request("during-failed-cleanup"), actor())
            .await,
        Err(AgentError::AttachmentUnavailable(AttachmentPhase::Failed(
            _
        )))
    ));
    assert!(matches!(
        agent
            .enqueue_steering(request("steering-during-failed-cleanup"), actor())
            .await,
        Err(AgentError::AttachmentUnavailable(AttachmentPhase::Failed(
            _
        )))
    ));
    release.send(()).unwrap();
    assert_eq!(attachment.wait().await, Err(AgentError::Closed));
}

#[tokio::test]
async fn provider_publication_preserves_queue_changes_made_while_open_waits() {
    let storage = MemoryStorage::default();
    let inner = TestProvider::new();
    let (release, waiting) = oneshot::channel();
    let provider = Arc::new(GatedOpenProvider {
        inner,
        entered: Notify::new(),
        release: Mutex::new(Some(waiting)),
    });
    let agent = prepared(provider.clone(), &storage, Arc::new(AcceptingAudit)).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    provider.entered.notified().await;

    let first = agent.enqueue(request("open-first"), actor()).await.unwrap();
    let removed = agent
        .enqueue(request("open-removed"), actor())
        .await
        .unwrap();
    let last = agent.enqueue(request("open-last"), actor()).await.unwrap();
    assert_eq!(
        agent
            .remove_queued(request("open-removed").execution_id, actor())
            .await
            .unwrap(),
        QueueRemoval::Removed
    );
    assert_eq!(
        agent
            .reorder_queued(
                vec![
                    request("open-last").execution_id,
                    request("open-first").execution_id,
                ],
                actor(),
            )
            .await
            .unwrap(),
        QueueReorder::Applied
    );

    release.send(()).unwrap();
    attachment.wait().await.unwrap();
    assert_eq!(removed.wait().await, Err(AgentError::Closed));
    assert_eq!(last.wait().await, Ok(ExecutionOutcome::Completed));
    assert_eq!(first.wait().await, Ok(ExecutionOutcome::Completed));
    let snapshot = storage.snapshot();
    assert!(matches!(
        snapshot.provider_context,
        ProviderContext::Recorded(_)
    ));
    assert!(snapshot.queue_history.iter().any(|record| {
        matches!(
            &record.mutation,
            QueueMutation::Removed { id, .. } if id.as_str() == "open-removed"
        )
    }));
    assert!(snapshot
        .queue_history
        .iter()
        .any(|record| matches!(&record.mutation, QueueMutation::Reordered(_))));
}

#[tokio::test]
async fn journal_restores_queue_changes_published_during_gated_open() {
    let root = tempdir().unwrap();
    let storage = Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap());
    let manager = SessionManager::open(
        Some(SessionId::new("gated-open-journal").unwrap()),
        storage.clone(),
    )
    .await
    .unwrap();
    let inner = TestProvider::new();
    let (release, waiting) = oneshot::channel();
    let provider = Arc::new(GatedOpenProvider {
        inner,
        entered: Notify::new(),
        release: Mutex::new(Some(waiting)),
    });
    let agent = Agent::prepare(provider.clone(), manager, Arc::new(AcceptingAudit))
        .await
        .unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    provider.entered.notified().await;
    let first = agent
        .enqueue(request("journal-first"), actor())
        .await
        .unwrap();
    let removed = agent
        .enqueue(request("journal-removed"), actor())
        .await
        .unwrap();
    let last = agent
        .enqueue(request("journal-last"), actor())
        .await
        .unwrap();
    assert_eq!(
        agent
            .remove_queued(request("journal-removed").execution_id, actor())
            .await
            .unwrap(),
        QueueRemoval::Removed
    );
    agent
        .reorder_queued(
            vec![
                request("journal-last").execution_id,
                request("journal-first").execution_id,
            ],
            actor(),
        )
        .await
        .unwrap();
    release.send(()).unwrap();
    attachment.wait().await.unwrap();
    assert!(removed.wait().await.is_err());
    last.wait().await.unwrap();
    first.wait().await.unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);

    let restored = Agent::prepare(
        provider,
        SessionManager::open(Some(SessionId::new("gated-open-journal").unwrap()), storage)
            .await
            .unwrap(),
        Arc::new(AcceptingAudit),
    )
    .await
    .unwrap();
    let snapshot = restored.session_manager().snapshot().await.unwrap();
    assert!(snapshot.queue_history.iter().any(|record| {
        matches!(
            &record.mutation,
            QueueMutation::Removed { id, .. } if id.as_str() == "journal-removed"
        )
    }));
    assert!(snapshot
        .queue_history
        .iter()
        .any(|record| matches!(&record.mutation, QueueMutation::Reordered(_))));
}

#[tokio::test]
async fn authorization_wait_and_start_publish_one_coherent_status() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = prepared(provider.clone(), &storage, Arc::new(AcceptingAudit)).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let status = agent.attachment_status();
    assert_eq!(status.phase(), AttachmentPhase::Waiting);
    assert_eq!(status.failure(), None);
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let status = agent.attachment_status();
    assert_eq!(status.phase(), AttachmentPhase::Attached);
    assert_eq!(status.failure(), None);
    assert_eq!(provider.calls.opens.lock().unwrap().len(), 1);
    assert!(matches!(
        storage.snapshot().provider_context,
        ProviderContext::Recorded(_)
    ));
}

#[tokio::test]
async fn close_before_attachment_task_runs_preserves_close_without_failed_transition() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let audit = Arc::new(AttachmentAuditProbe::default());
    *audit.gate_after.lock().unwrap() = Some(AttachmentAuditStage::Starting);
    let (release, waiting) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting);
    let agent = prepared(provider.clone(), &storage, audit.clone()).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    audit.entered.notified().await;

    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(close_action()).await }
    });
    while agent.attachment_status().phase() != AttachmentPhase::Absent {
        tokio::task::yield_now().await;
    }
    assert!(!closing.is_finished());
    release.send(()).unwrap();
    closing.await.unwrap().unwrap();
    assert_eq!(attachment.wait().await, Err(AgentError::Closed));
    assert!(provider.calls.opens.lock().unwrap().is_empty());
    assert!(provider.calls.closes.lock().unwrap().is_empty());
    assert!(audit
        .records
        .lock()
        .unwrap()
        .iter()
        .all(|record| record.after() != AttachmentAuditStage::Failed));
    let records = audit.records.lock().unwrap();
    assert!(records.iter().any(|record| {
        record.cause() == AttachmentAuditCause::Closed
            && record.generation() == 0
            && record.before() == AttachmentAuditStage::Starting
            && record.after() == AttachmentAuditStage::Absent
            && record.actor() == Some(&close_action())
    }));
}

#[tokio::test]
async fn caller_loss_while_started_audit_is_pending_keeps_close_evidence_owned() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let audit = Arc::new(AttachmentAuditProbe::default());
    *audit.gate_after.lock().unwrap() = Some(AttachmentAuditStage::Starting);
    let (release, waiting) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting);
    let agent = prepared(provider.clone(), &storage, audit.clone()).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    audit.entered.notified().await;

    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(close_action()).await }
    });
    while agent.attachment_status().phase() != AttachmentPhase::Absent {
        tokio::task::yield_now().await;
    }
    closing.abort();
    let _ = closing.await;
    assert!(agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .is_err());

    release.send(()).unwrap();
    assert_eq!(attachment.wait().await, Err(AgentError::Closed));
    agent.close(close_action()).await.unwrap();
    assert!(provider.calls.opens.lock().unwrap().is_empty());
}

#[tokio::test]
async fn published_context_is_not_dispatchable_before_audit_acknowledgement() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let audit = Arc::new(AttachmentAuditProbe::default());
    audit.reject.store(true, Ordering::SeqCst);
    *audit.reject_after.lock().unwrap() = Some(AttachmentAuditStage::ContextPublished);
    *audit.gate_after.lock().unwrap() = Some(AttachmentAuditStage::ContextPublished);
    let (release, waiting) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting);
    let agent = prepared(provider.clone(), &storage, audit.clone()).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    audit.entered.notified().await;

    let status = agent.attachment_status();
    assert_eq!(status.phase(), AttachmentPhase::Starting);
    assert_eq!(
        agent.invoke(request("direct-before-audit"), actor()).await,
        Err(AgentError::AttachmentUnavailable(AttachmentPhase::Starting))
    );
    let queued = agent
        .enqueue(request("queued-before-audit"), actor())
        .await
        .unwrap();
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);

    release.send(()).unwrap();
    assert_eq!(attachment.wait().await, Err(AgentError::AuditFailure));
    assert!(queued.wait().await.is_err());
    assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        agent
            .attachment_status()
            .evidence_failure()
            .map(AttachmentFailure::code),
        Some(AttachmentFailureCode::Audit)
    );
}

#[tokio::test]
async fn close_joins_context_publication_registered_before_delivery() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let audit = Arc::new(AttachmentAuditProbe::default());
    *audit.gate_after.lock().unwrap() = Some(AttachmentAuditStage::ContextPublished);
    let (release, waiting) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting);
    let agent = prepared(provider.clone(), &storage, audit.clone()).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    audit.entered.notified().await;

    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(close_action()).await }
    });
    while agent.attachment_status().phase() != AttachmentPhase::Absent {
        tokio::task::yield_now().await;
    }
    assert!(!closing.is_finished());
    release.send(()).unwrap();
    closing.await.unwrap().unwrap();
    assert_eq!(attachment.wait().await, Err(AgentError::Closed));
    assert_eq!(provider.calls.closes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn close_joins_failed_transition_registered_before_delivery() {
    let storage = MemoryStorage::default();
    let audit = Arc::new(AttachmentAuditProbe::default());
    *audit.gate_after.lock().unwrap() = Some(AttachmentAuditStage::Failed);
    let (release, waiting) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting);
    let agent = Agent::prepare(
        Arc::new(NoResourcesProvider),
        storage.manager().await,
        audit.clone(),
    )
    .await
    .unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    audit.entered.notified().await;

    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(close_action()).await }
    });
    while agent.attachment_status().phase()
        != AttachmentPhase::Failed(AttachmentFailureCode::Provider)
    {
        tokio::task::yield_now().await;
    }
    assert!(!closing.is_finished());
    release.send(()).unwrap();
    closing.await.unwrap().unwrap();
    assert_eq!(attachment.wait().await, Err(AgentError::Closed));
}

#[tokio::test]
async fn close_fences_and_wakes_only_its_held_authorization() {
    let first_storage = MemoryStorage::default();
    let second_storage = MemoryStorage::default();
    let first = prepared(
        TestProvider::new(),
        &first_storage,
        Arc::new(AcceptingAudit),
    )
    .await;
    let second = prepared(
        TestProvider::new(),
        &second_storage,
        Arc::new(AcceptingAudit),
    )
    .await;
    let first_authorization = first
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let second_authorization = second
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let first_cancelled = first_authorization.cancellation().wait();
    let second_cancelled = second_authorization.cancellation().wait();
    tokio::pin!(first_cancelled, second_cancelled);
    let closing = tokio::spawn({
        let first = first.clone();
        async move { first.close(close_action()).await }
    });
    tokio::time::timeout(Duration::from_secs(1), &mut first_cancelled)
        .await
        .expect("matching authorization woke");
    tokio::select! {
        biased;
        _ = &mut second_cancelled => panic!("unrelated authorization woke"),
        _ = tokio::task::yield_now() => {}
    }
    assert!(matches!(
        first.start_attachment(first_authorization),
        Err(AgentError::AttachmentAuthorizationStale)
    ));
    closing.await.unwrap().unwrap();
    second
        .start_attachment(second_authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn attachment_authorization_cannot_be_consumed_by_an_identical_other_agent() {
    let first_storage = MemoryStorage::default();
    let second_storage = MemoryStorage::default();
    let first = prepared(
        TestProvider::new(),
        &first_storage,
        Arc::new(AcceptingAudit),
    )
    .await;
    let second_audit = Arc::new(AttachmentAuditProbe::default());
    let (release, waiting) = oneshot::channel();
    *second_audit.release.lock().unwrap() = Some(waiting);
    let second = prepared(TestProvider::new(), &second_storage, second_audit.clone()).await;
    let first_authorization = first
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let second_authorization = second
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();

    assert!(matches!(
        first.start_attachment(second_authorization),
        Err(AgentError::AttachmentAuthorizationStale)
    ));
    second_audit.entered.notified().await;
    assert_eq!(first.attachment_status().phase(), AttachmentPhase::Waiting);
    first
        .start_attachment(first_authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(matches!(
        second.authorize_attachment(AttachmentRequest::CallerRequested(actor())),
        Err(AgentError::AttachmentAuthorizationStale)
    ));
    release.send(()).unwrap();
    second_audit.completed.notified().await;
    let replacement = second
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    drop(replacement);
}

#[tokio::test]
async fn close_of_unopened_authorization_records_generation_and_closer_once() {
    let storage = MemoryStorage::default();
    let audit = Arc::new(AttachmentAuditProbe::default());
    let agent = prepared(TestProvider::new(), &storage, audit.clone()).await;
    let _authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();

    agent.close(close_action()).await.unwrap();
    agent.close(close_action()).await.unwrap();

    let closed = audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter(|record| record.cause() == AttachmentAuditCause::Closed)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].generation(), 0);
    assert_eq!(closed[0].before(), AttachmentAuditStage::Waiting);
    assert_eq!(closed[0].after(), AttachmentAuditStage::Absent);
    assert_eq!(closed[0].actor(), Some(&close_action()));
}

#[tokio::test]
async fn rejecting_unopened_close_audit_remains_visible_and_blocks_success() {
    let storage = MemoryStorage::default();
    let audit = Arc::new(AttachmentAuditProbe::default());
    audit.reject.store(true, Ordering::SeqCst);
    *audit.reject_after.lock().unwrap() = Some(AttachmentAuditStage::Absent);
    let agent = prepared(TestProvider::new(), &storage, audit).await;
    let _authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();

    assert_eq!(
        agent.close(close_action()).await,
        Err(AgentError::AuditFailure)
    );
    assert_eq!(
        agent
            .attachment_status()
            .evidence_failure()
            .map(AttachmentFailure::code),
        Some(AttachmentFailureCode::Audit)
    );
    assert_eq!(
        agent.close(close_action()).await,
        Err(AgentError::AuditFailure)
    );
}

#[tokio::test]
async fn rejecting_starting_close_audit_blocks_close_and_retains_the_failed_generation() {
    let storage = MemoryStorage::default();
    let audit = Arc::new(AttachmentAuditProbe::default());
    audit.reject.store(true, Ordering::SeqCst);
    *audit.reject_after.lock().unwrap() = Some(AttachmentAuditStage::Absent);
    *audit.gate_after.lock().unwrap() = Some(AttachmentAuditStage::Starting);
    let (release, waiting) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting);
    let agent = prepared(TestProvider::new(), &storage, audit.clone()).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    audit.entered.notified().await;

    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(close_action()).await }
    });
    while agent.attachment_status().phase() != AttachmentPhase::Absent {
        tokio::task::yield_now().await;
    }
    assert!(!closing.is_finished());
    release.send(()).unwrap();
    assert_eq!(closing.await.unwrap(), Err(AgentError::AuditFailure));
    assert_eq!(attachment.wait().await, Err(AgentError::Closed));
    let status = agent.attachment_status();
    let failed = status.evidence_failure().unwrap();
    assert_eq!(failed.generation(), 0);
    assert_eq!(failed.code(), AttachmentFailureCode::Audit);
}

#[tokio::test]
async fn automatic_stop_waits_for_and_retains_rejected_starting_close_audit() {
    let storage = MemoryStorage::default();
    let (start_release, start_waiting) = oneshot::channel();
    let (close_release, close_waiting) = oneshot::channel();
    let audit = Arc::new(AutomaticCloseAudit {
        starting_entered: Notify::new(),
        starting_release: Mutex::new(Some(start_waiting)),
        closed_entered: Notify::new(),
        closed_release: Mutex::new(Some(close_waiting)),
    });
    let agent = prepared(TestProvider::new(), &storage, audit.clone()).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    audit.starting_entered.notified().await;
    let admission = agent
        .enqueue(request("automatic-close-audit"), actor())
        .await
        .unwrap();
    audit.closed_entered.notified().await;
    assert!(agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .is_err());

    close_release.send(()).unwrap();
    start_release.send(()).unwrap();
    assert!(admission.wait().await.is_err());
    assert_eq!(attachment.wait().await, Err(AgentError::Closed));
    tokio::task::yield_now().await;
    assert!(agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .is_err());
    assert_eq!(
        agent
            .attachment_status()
            .evidence_failure()
            .map(AttachmentFailure::code),
        Some(AttachmentFailureCode::Audit)
    );
}

#[tokio::test]
async fn caller_loss_does_not_abandon_unopened_close_evidence() {
    let storage = MemoryStorage::default();
    let audit = Arc::new(AttachmentAuditProbe::default());
    *audit.gate_after.lock().unwrap() = Some(AttachmentAuditStage::Absent);
    let (release, waiting) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting);
    let agent = prepared(TestProvider::new(), &storage, audit.clone()).await;
    let _authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(close_action()).await }
    });
    audit.entered.notified().await;
    closing.abort();
    let _ = closing.await;
    release.send(()).unwrap();
    audit.completed.notified().await;

    assert!(audit
        .records
        .lock()
        .unwrap()
        .iter()
        .any(|record| record.cause() == AttachmentAuditCause::Closed));
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn panicking_unopened_close_audit_is_reported_for_every_future_boundary() {
    for panic in [AuditPanic::Construct, AuditPanic::Poll, AuditPanic::Drop] {
        let storage = MemoryStorage::default();
        let agent = prepared(
            TestProvider::new(),
            &storage,
            Arc::new(PanickingAudit(panic)),
        )
        .await;
        let _authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        assert_eq!(
            agent.close(close_action()).await,
            Err(AgentError::AuditFailure)
        );
    }
}

#[tokio::test]
async fn abandoned_authorization_blocks_replacement_until_audit_settles() {
    let storage = MemoryStorage::default();
    let audit = Arc::new(AttachmentAuditProbe::default());
    let (release, waiting) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting);
    let agent = prepared(TestProvider::new(), &storage, audit.clone()).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    drop(authorization);
    audit.entered.notified().await;
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Waiting);
    assert!(matches!(
        agent.authorize_attachment(AttachmentRequest::CallerRequested(actor())),
        Err(AgentError::AttachmentAuthorizationStale)
    ));
    release.send(()).unwrap();
    audit.completed.notified().await;
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Absent);
}

#[tokio::test]
async fn superseded_abandonment_failure_remains_visible_without_revival() {
    let storage = MemoryStorage::default();
    let audit = Arc::new(AttachmentAuditProbe::default());
    audit.reject.store(true, Ordering::SeqCst);
    let (release, waiting) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting);
    let agent = prepared(TestProvider::new(), &storage, audit.clone()).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    drop(authorization);
    audit.entered.notified().await;
    let closing = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(close_action()).await }
    });
    while agent.attachment_status().phase() == AttachmentPhase::Waiting {
        tokio::task::yield_now().await;
    }
    release.send(()).unwrap();
    assert_eq!(closing.await.unwrap(), Err(AgentError::AuditFailure));
    audit.completed.notified().await;
    let status = agent.attachment_status();
    assert_ne!(status.phase(), AttachmentPhase::Attached);
    assert_eq!(
        status.evidence_failure().map(AttachmentFailure::code),
        Some(AttachmentFailureCode::Audit)
    );
}

#[tokio::test]
async fn attachment_audit_construction_poll_and_drop_panics_settle_failed() {
    for failure in [AuditPanic::Construct, AuditPanic::Poll, AuditPanic::Drop] {
        let storage = MemoryStorage::default();
        let agent = prepared(
            TestProvider::new(),
            &storage,
            Arc::new(PanickingAudit(failure)),
        )
        .await;
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        assert_eq!(
            agent.start_attachment(authorization).unwrap().wait().await,
            Err(AgentError::AuditFailure)
        );
        let status = agent.attachment_status();
        assert_eq!(
            status.phase(),
            AttachmentPhase::Failed(AttachmentFailureCode::Audit)
        );
        assert_eq!(status.failure().unwrap().cause(), AttachmentCause::Initial);
    }
}

#[tokio::test]
async fn provider_open_construction_poll_and_drop_panics_settle_without_unwind() {
    for failure in [OpenPanic::Construct, OpenPanic::Poll, OpenPanic::Drop] {
        let storage = MemoryStorage::default();
        let agent = Agent::prepare(
            Arc::new(PanickingProvider(failure)),
            storage.manager().await,
            Arc::new(AcceptingAudit),
        )
        .await
        .unwrap();
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        assert!(agent
            .start_attachment(authorization)
            .unwrap()
            .wait()
            .await
            .is_err());
        assert!(matches!(
            agent.attachment_status().phase(),
            AttachmentPhase::Failed(_)
        ));
    }
}

#[tokio::test]
async fn cleanup_construction_poll_and_drop_panics_remain_retryable() {
    for failure in [
        CleanupPanic::Construct,
        CleanupPanic::Poll,
        CleanupPanic::Drop,
        CleanupPanic::DropAfterCompletionFailure,
    ] {
        let storage = MemoryStorage::default();
        let agent = Agent::prepare(
            Arc::new(PanickingCleanupProvider { failure }),
            storage.manager().await,
            Arc::new(AcceptingAudit),
        )
        .await
        .unwrap();
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        agent
            .start_attachment(authorization)
            .unwrap()
            .wait()
            .await
            .unwrap();

        let expected = if matches!(failure, CleanupPanic::DropAfterCompletionFailure) {
            AgentError::MultipleOperationFailures {
                first_error: Box::new(AgentError::Transport("cleanup completion failed".into())),
                subsequent_error: Box::new(AgentError::CleanupUncertain),
            }
        } else {
            AgentError::CleanupUncertain
        };
        assert_eq!(agent.close(close_action()).await, Err(expected.clone()));
        assert_eq!(
            agent.close(close_action()).await,
            if matches!(
                failure,
                CleanupPanic::Drop | CleanupPanic::DropAfterCompletionFailure
            ) {
                Err(expected)
            } else {
                Ok(CloseOutcome { forced: false })
            }
        );
        drop(agent);
        drop(storage.manager().await);
    }
}

#[tokio::test]
async fn provider_context_save_panics_fail_durably_and_cleanup_provider() {
    for failure in [SavePanic::Construct, SavePanic::Poll, SavePanic::Drop] {
        let backing = MemoryStorage::default();
        let failures = Arc::new(Mutex::new(None));
        let storage = Arc::new(PanickingStorage {
            backing: backing.clone(),
            failure: failures.clone(),
        });
        let manager = SessionManager::open(
            Some(SessionId::new("panic-save-conversation").unwrap()),
            storage,
        )
        .await
        .unwrap();
        let provider = TestProvider::new();
        let audit = Arc::new(AttachmentAuditProbe::default());
        let agent = Agent::prepare(provider.clone(), manager, audit.clone())
            .await
            .unwrap();
        *failures.lock().unwrap() = Some(failure);
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        assert!(agent
            .start_attachment(authorization)
            .unwrap()
            .wait()
            .await
            .is_err());
        assert_eq!(provider.calls.opens.lock().unwrap().len(), 1);
        assert_eq!(provider.calls.closes.lock().unwrap().len(), 1);
        assert!(matches!(
            agent.attachment_status().phase(),
            AttachmentPhase::Failed(AttachmentFailureCode::Storage)
        ));
        let retry = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        agent.start_attachment(retry).unwrap().wait().await.unwrap();
        assert_eq!(provider.calls.opens.lock().unwrap().len(), 2);
        assert!(provider.calls.opens.lock().unwrap()[1].is_some());
        assert!(audit.records.lock().unwrap().iter().any(|record| {
            record.cause() == AttachmentAuditCause::Started(AttachmentCause::Reopen)
        }));
        agent.close(close_action()).await.unwrap();
    }
}

#[tokio::test]
async fn queue_audit_panics_preserve_owned_receipt_and_prevent_dispatch() {
    for failure in [AuditPanic::Construct, AuditPanic::Poll, AuditPanic::Drop] {
        let storage = MemoryStorage::default();
        let provider = TestProvider::new();
        let agent = Agent::prepare(
            provider.clone(),
            storage.manager().await,
            Arc::new(QueuePanickingAudit(failure)),
        )
        .await
        .unwrap();
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        agent
            .start_attachment(authorization)
            .unwrap()
            .wait()
            .await
            .unwrap();

        let admission = agent
            .enqueue(request("queue-audit-panic"), actor())
            .await
            .unwrap();
        let original_evidence = admission.evidence().clone();
        assert!(
            matches!(admission.evidence(), AdmissionEvidence::Failed(failure)
            if failure.audit() == Some(&AgentError::AuditFailure))
        );
        assert!(admission.wait().await.is_err());
        assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
        agent.close(close_action()).await.unwrap();
        drop(agent);
        let restored = attached_agent(provider.clone(), storage.manager().await)
            .await
            .unwrap();
        let retry = restored
            .enqueue(request("queue-audit-panic"), actor())
            .await
            .unwrap();
        assert_eq!(retry.evidence(), &original_evidence);
        assert!(retry.wait().await.is_err());
        assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
        restored.close(close_action()).await.unwrap();
    }
}

#[path = "initialization/startup_control.rs"]
mod startup_control;
