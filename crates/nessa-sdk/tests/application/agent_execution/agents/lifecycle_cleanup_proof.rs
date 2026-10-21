//! New mandatory evidence waits behind the lifecycle's admitted proof writer.
use super::*;
use crate::application::agent_execution::{
    executions::{ExecutionAudit, ExecutionAuditRecord},
    providers::CloseOutcome,
    AgentFuture,
};
use crate::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

struct CountingAudit(AtomicUsize);
impl ExecutionAudit for CountingAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn new_attachment_evidence_waits_for_admitted_cleanup_proof_writer() {
    let manager = Arc::new(
        SessionManager::open(
            None,
            Arc::new(InMemoryStorage::new()),
            Arc::new(RuntimeMessageCommitClock::new()),
        )
        .await
        .unwrap(),
    );
    let audit = Arc::new(CountingAudit(AtomicUsize::new(0)));
    let lifecycle = SessionLifecycle::new(
        manager.attachment(),
        manager.clone(),
        false,
        manager.id().clone(),
        Arc::new(AuditObserver::new(
            audit.clone(),
            manager.message_commit_clock(),
        )),
    );
    let ticket = CloseAttempt::from_physical_report(
        0,
        0,
        SessionCloseRequest::SessionFailed,
        CleanupReport::confirmed(CloseOutcome { forced: false }),
        vec![],
        0,
        0,
    );
    let (entered, writing) = oneshot::channel();
    let (release, held) = oneshot::channel();
    let owner = lifecycle.clone();
    let writer = tokio::spawn(async move {
        owner
            .record_cleanup_proof(&ticket, async {
                entered.send(()).unwrap();
                held.await.unwrap();
                Ok(())
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), writing)
        .await
        .unwrap()
        .unwrap();
    let record = ExecutionAuditRecord::Attachment(AttachmentAuditRecord {
        session_id: manager.id().clone(),
        generation: 0,
        before: AttachmentAuditStage::Waiting,
        after: AttachmentAuditStage::Absent,
        cause: AttachmentAuditCause::AuthorizationAbandoned,
        actor: Some(ActionContext::new("owner", "test", "abandoned").unwrap()),
    });
    let mut evidence = Box::pin(lifecycle.record_new_attachment_evidence(record));
    let pending =
        std::future::poll_fn(|context| Poll::Ready(evidence.as_mut().poll(context).is_pending()))
            .await;
    assert!(
        pending,
        "new mandatory evidence cannot pass an admitted proof writer"
    );
    assert_eq!(
        audit.0.load(Ordering::SeqCst),
        0,
        "no audit effect before the original writer ACK"
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), evidence)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audit.0.load(Ordering::SeqCst), 1);
}
