//! Prepared construction and attachment ownership use separate observable phases.
use super::*;
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::AtomicBool,
    task::{Context, Poll},
};
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
                    .map_or(true, |stage| stage == record.after());
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
                        .map_or(true, |stage| stage == after),
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

#[derive(Clone, Copy)]
enum OpenPanic {
    Construct,
    Poll,
    Drop,
}
struct PanickingProvider(OpenPanic);
struct PanickingOpen(OpenPanic);
#[derive(Clone, Copy)]
enum CleanupPanic {
    Construct,
    Poll,
    Drop,
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
    fn open(&self, _restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        if matches!(self.0, OpenPanic::Construct) {
            panic!("open construction panic");
        }
        Box::pin(PanickingOpen(self.0))
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
    fn open(&self, _restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
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
        Poll::Ready(CleanupReport::confirmed(CloseOutcome { forced: false }))
    }
}
impl Drop for PanickingCleanupFuture {
    fn drop(&mut self) {
        if matches!(self.0, CleanupPanic::Drop) {
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
    provider: Arc<TestProvider>,
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

    agent.close(close_action()).await.unwrap();
    release.send(()).unwrap();
    assert_eq!(attachment.wait().await, Err(AgentError::Closed));
    assert!(provider.calls.opens.lock().unwrap().is_empty());
    assert!(provider.calls.closes.lock().unwrap().is_empty());
    assert!(audit
        .records
        .lock()
        .unwrap()
        .iter()
        .all(|record| record.after() != AttachmentAuditStage::Failed));
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

        assert_eq!(
            agent.close(close_action()).await,
            Err(AgentError::CleanupUncertain)
        );
        agent.close(close_action()).await.unwrap();
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
        let agent = Agent::prepare(provider.clone(), manager, Arc::new(AcceptingAudit))
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
        assert!(
            matches!(admission.evidence(), AdmissionEvidence::Failed(failure)
            if failure.audit() == Some(&AgentError::AuditFailure))
        );
        assert!(admission.wait().await.is_err());
        assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
    }
}
