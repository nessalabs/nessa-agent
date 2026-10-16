//! Admission belongs to the retained submit owner, including receipt persistence.
use super::super::super::Halt;
use super::*;
use nessa_sdk::application::agent_execution::commands::{
    CommandAccess, CreationReceipt, CreationStorageLease,
};
use std::future::{poll_fn, Future};
use std::task::Poll;
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinHandle;

const HANG: Duration = Duration::from_secs(20);

struct ReceiptGate {
    stage: MutationStage,
    entered: Notify,
    release: Notify,
}
impl ReceiptGate {
    fn at(stage: MutationStage) -> Arc<Self> {
        Arc::new(Self {
            stage,
            entered: Notify::new(),
            release: Notify::new(),
        })
    }
    async fn wait(&self) {
        tokio::time::timeout(HANG, self.entered.notified())
            .await
            .expect("receipt save reached the gate");
    }
}
struct GatedCommands {
    real: Arc<RecordStorage>,
    gate: Arc<ReceiptGate>,
}
struct GatedLease {
    real: Arc<dyn CreationStorageLease>,
    gate: Arc<ReceiptGate>,
}
impl CreationStorage for GatedCommands {
    fn open_command(
        &self,
        principal: String,
        request_id: String,
        access: CommandAccess,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async move {
            Ok(self
                .real
                .open_command(principal, request_id, access)
                .await?
                .map(|real| {
                    Arc::new(GatedLease {
                        real,
                        gate: self.gate.clone(),
                    }) as Arc<dyn CreationStorageLease>
                }))
        })
    }
}
impl CreationStorageLease for GatedLease {
    fn load(&self, request: &str) -> CreationFuture<'_, Option<CreationReceipt>> {
        self.real.load(request)
    }
    fn save(&self, receipt: CreationReceipt) -> CreationFuture<'_, ()> {
        self.real.save(receipt)
    }
    fn load_mutation(&self, request: &str) -> CreationFuture<'_, Option<MutationReceipt>> {
        self.real.load_mutation(request)
    }
    fn save_mutation(&self, receipt: MutationReceipt) -> CreationFuture<'_, ()> {
        Box::pin(async move {
            let held = receipt.stage() == self.gate.stage;
            self.real.save_mutation(receipt).await?;
            if held {
                self.gate.entered.notify_one();
                self.gate.release.notified().await;
            }
            Ok(())
        })
    }
}

fn submit(
    service: &ConversationService,
    storage: &Arc<RecordStorage>,
    target: &ConversationId,
    gate: &Arc<ReceiptGate>,
    mode: SubmissionMode,
) -> JoinHandle<Result<SubmittedCommand, MutationFailure<ConversationError>>> {
    let service = service.clone();
    let storage = Arc::new(GatedCommands {
        real: storage.clone(),
        gate: gate.clone(),
    });
    let target = target.clone();
    tokio::spawn(async move {
        service
            .submit_command(
                storage,
                target,
                caller("submit"),
                Initiation::owner(),
                true,
                "turn".into(),
                message("hello"),
                mode,
            )
            .await
    })
}

#[tokio::test]
async fn admitted_submit_finishes_before_waiting_export() {
    for mode in [SubmissionMode::Queue, SubmissionMode::Steer] {
        let directory = tempfile::tempdir().unwrap();
        let provider = Arc::new(ProviderFactory::default());
        provider.native_steering.store(true, Ordering::SeqCst);
        let (service, storage, metadata) =
            fixture(&directory.path().join("data"), provider.clone());
        let target = id();
        opened(&service, &provider, &target).await;
        let gate = ReceiptGate::at(MutationStage::Attempted);
        let task = submit(&service, &storage, &target, &gate, mode);
        gate.wait().await;
        // Poll this exact writer before allowing enqueue: Tokio's fair lock
        // queues it behind the command's original reader and ahead of new reads.
        let mut export = Box::pin(service.hold_for_export());
        poll_fn(|cx| {
            assert!(export.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        gate.release.notify_one();
        let receipt = tokio::time::timeout(HANG, task)
            .await
            .expect("submit completes without cancelling the queued export")
            .unwrap()
            .unwrap();
        assert_eq!(
            receipt.progress.outcome(),
            Some(MutationOutcome::Dispatched)
        );
        assert!(receipt.delivery.is_some());
        let hold = tokio::time::timeout(HANG, export).await.unwrap().unwrap();
        drop(hold);
        let retried = service
            .submit_command(
                storage.clone(),
                target,
                caller("submit"),
                Initiation::owner(),
                true,
                "turn".into(),
                message("hello"),
                mode,
            )
            .await
            .unwrap();
        assert_eq!(retried.progress, receipt.progress);
        assert!(retried.delivery.is_none());
        tokio::time::timeout(HANG, provider.execution_started.notified())
            .await
            .unwrap();
        assert_eq!(provider.executions.lock().unwrap().as_slice(), ["turn"]);
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
        retire(service, storage, metadata).await;
    }
}

#[tokio::test]
async fn caller_loss_keeps_submit_admission_until_settlement() {
    for mode in [SubmissionMode::Queue, SubmissionMode::Steer] {
        let directory = tempfile::tempdir().unwrap();
        let provider = Arc::new(ProviderFactory::default());
        let (service, storage, metadata) =
            fixture(&directory.path().join("data"), provider.clone());
        let target = id();
        opened(&service, &provider, &target).await;
        let gate = ReceiptGate::at(MutationStage::Settled);
        let task = submit(&service, &storage, &target, &gate, mode);
        gate.wait().await;
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        let mut export = Box::pin(service.hold_for_export());
        poll_fn(|cx| {
            assert!(export.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        gate.release.notify_one();
        let hold = tokio::time::timeout(HANG, export).await.unwrap().unwrap();
        drop(hold);
        let retried = service
            .submit_command(
                storage.clone(),
                target,
                caller("submit"),
                Initiation::owner(),
                true,
                "turn".into(),
                message("hello"),
                mode,
            )
            .await
            .unwrap();
        assert_eq!(
            retried.progress.outcome(),
            Some(MutationOutcome::Dispatched)
        );
        assert!(retried.delivery.is_none());
        tokio::time::timeout(HANG, provider.execution_started.notified())
            .await
            .unwrap();
        assert_eq!(provider.executions.lock().unwrap().as_slice(), ["turn"]);
        retire(service, storage, metadata).await;
    }
}

#[tokio::test]
async fn retirement_before_submit_effect_preserves_attempted_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let gate = ReceiptGate::at(MutationStage::Attempted);
    let task = submit(&service, &storage, &target, &gate, SubmissionMode::Queue);
    gate.wait().await;
    let mode = service.inner.mode_changes.lock(&target).await;
    let mut retirement = Box::pin(service.retire("test", "retire"));
    poll_fn(|cx| {
        assert!(retirement.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    gate.release.notify_one();
    assert!(matches!(
        tokio::time::timeout(HANG, task).await.unwrap().unwrap(),
        Err(MutationFailure::Target(ConversationError::Unavailable))
    ));
    drop(mode);
    tokio::time::timeout(HANG, retirement)
        .await
        .unwrap()
        .unwrap();
    assert!(provider.executions.lock().unwrap().is_empty());
    let lease = storage
        .open_command("person".into(), "submit".into(), CommandAccess::ReadOnly)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lease
            .load_mutation("submit")
            .await
            .unwrap()
            .unwrap()
            .stage(),
        MutationStage::Attempted
    );
    drop(lease);
    retire(service, storage, metadata).await;
    let restored_provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) =
        fixture(&directory.path().join("data"), restored_provider.clone());
    assert!(matches!(
        service
            .submit_command(
                storage.clone(), target, caller("submit"), Initiation::owner(), true,
                "turn".into(), message("hello"), SubmissionMode::Queue,
            )
            .await,
        Err(MutationFailure::Interrupted(receipt)) if receipt.stage() == MutationStage::Attempted
    ));
    assert_eq!(restored_provider.open_calls.load(Ordering::SeqCst), 0);
    assert!(restored_provider.executions.lock().unwrap().is_empty());
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn submit_waits_while_export_holds_admission() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let hold = service.hold_for_export().await.unwrap();
    let mode = service.inner.mode_changes.lock(&target).await;
    // The durable command begins with this same admission operation. Poll it
    // directly so the assertion proves a queued reader, rather than an async
    // caller that might not yet have scheduled its retained task.
    let mut admission = Box::pin(service.admit());
    let (asked, submission) = service.submission_future(
        target.clone(),
        caller("ordinary"),
        "ordinary".into(),
        message("hello"),
        SubmissionMode::Queue,
        Writer::Person(Initiation::owner(), true),
        None,
    );
    let mut submission = Box::pin(submission);
    poll_fn(|cx| {
        assert!(admission.as_mut().poll(cx).is_pending());
        assert!(
            std::pin::pin!(tokio::task::unconstrained(submission.as_mut()))
                .poll(cx)
                .is_pending()
        );
        Poll::Ready(())
    })
    .await;
    assert!(!asked.load(Ordering::SeqCst));
    assert_eq!(
        service.inner.mode_changes.holders_and_waiters(&target),
        1,
        "submission has not reached the mode lock behind admission"
    );
    assert!(provider.executions.lock().unwrap().is_empty());
    assert_eq!(command_rows(&directory.path().join("data")), 0);
    drop(hold);
    drop(
        tokio::time::timeout(HANG, admission)
            .await
            .unwrap()
            .unwrap(),
    );
    drop(mode);
    assert!(tokio::time::timeout(HANG, submission)
        .await
        .unwrap()
        .is_ok());
    assert!(asked.load(Ordering::SeqCst));
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn submission_refuses_another_services_admission() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let foreign = Arc::new(RwLock::new(())).read_owned().await;
    let (asked, submission) = service.submission_future(
        id(),
        caller("foreign"),
        "turn".into(),
        message("hello"),
        SubmissionMode::Queue,
        Writer::Person(Initiation::owner(), true),
        Some(Arc::new(foreign)),
    );
    assert!(matches!(
        submission.await,
        Err(Halt::Failed(ConversationError::Unavailable))
    ));
    assert!(!asked.load(Ordering::SeqCst));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}
