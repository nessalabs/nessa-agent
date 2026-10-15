//! A stalled queue audit cannot hold back physical cleanup of an attached provider.
use super::*;
use nessa_sdk::application::agent_execution::{
    agents::{AdmissionEvidence, AgentFuture},
    executions::{ExecutionAudit, ExecutionAuditRecord},
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

struct PendingQueueAdmissionAudit(Mutex<Option<oneshot::Sender<()>>>);
impl ExecutionAudit for PendingQueueAdmissionAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            if matches!(record, ExecutionAuditRecord::QueueAdmitted(_)) {
                if let Some(entered) = self.0.lock().unwrap().take() {
                    entered.send(()).unwrap();
                }
                return std::future::pending().await;
            }
            Ok(())
        })
    }
}

#[tokio::test(start_paused = true)]
async fn pending_queue_audit_does_not_delay_attached_provider_cleanup_or_dispatch_input() {
    let storage = MemoryStorage::default();
    let (provider, backend) = probe_factory(false);
    let (entered, waiting) = oneshot::channel();
    let agent = Agent::prepare(
        Arc::new(provider),
        storage.manager().await,
        Arc::new(PendingQueueAdmissionAudit(Mutex::new(Some(entered)))),
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
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Attached);

    let admission = tokio::spawn({
        let agent = agent.clone();
        async move {
            agent
                .enqueue(input("stalled-audit"), Submitter::owner(actor()))
                .await
        }
    });
    waiting.await.unwrap();
    let close = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(actor()).await }
    });
    backend.closing.notified().await;
    assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
    assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
    assert!(!close.is_finished());
    tokio::time::advance(Duration::from_secs(30)).await;

    let admission = timeout(Duration::from_secs(1), admission)
        .await
        .expect("the queue audit deadline must release admission")
        .unwrap()
        .unwrap();
    assert!(
        matches!(admission.evidence(), AdmissionEvidence::Failed(failure)
        if failure.audit() == Some(&AgentError::AuditFailure))
    );
    assert_eq!(admission.wait().await, Err(AgentError::Closed));
    assert!(timeout(Duration::from_secs(1), close)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
    assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        *backend.close_requests.lock().unwrap(),
        vec![SessionCloseRequest::Explicit(actor())]
    );
}

struct PanickingQueueAudit {
    owner: Mutex<Option<Agent>>,
    fenced_before_drop: Arc<AtomicBool>,
}
struct PanickingQueueEvidence {
    owner: Agent,
    fenced_before_drop: Arc<AtomicBool>,
}
impl ExecutionAudit for PanickingQueueAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        if matches!(record, ExecutionAuditRecord::QueueAdmitted(_)) {
            Box::pin(PanickingQueueEvidence {
                owner: self.owner.lock().unwrap().take().unwrap(),
                fenced_before_drop: self.fenced_before_drop.clone(),
            })
        } else {
            Box::pin(async { Ok(()) })
        }
    }
}
impl Future for PanickingQueueEvidence {
    type Output = Result<(), AgentError>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        panic!("queue audit poll fault");
    }
}
impl Drop for PanickingQueueEvidence {
    fn drop(&mut self) {
        self.fenced_before_drop.store(
            matches!(
                self.owner
                    .authorize_attachment(AttachmentRequest::CallerRequested(actor())),
                Err(AgentError::Closed)
            ),
            Ordering::SeqCst,
        );
    }
}

#[tokio::test]
async fn scheduling_audit_poll_fault_fences_admission_before_capture_cleanup() {
    let storage = MemoryStorage::default();
    let (provider, backend) = probe_factory(false);
    let fenced = Arc::new(AtomicBool::new(false));
    let audit = Arc::new(PanickingQueueAudit {
        owner: Mutex::new(None),
        fenced_before_drop: fenced.clone(),
    });
    let agent = Agent::prepare(Arc::new(provider), storage.manager().await, audit.clone())
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
    *audit.owner.lock().unwrap() = Some(agent.clone());
    let admission = agent
        .enqueue(input("panic-fence"), Submitter::owner(actor()))
        .await
        .unwrap();
    assert!(
        matches!(admission.evidence(), AdmissionEvidence::Failed(failure)
        if failure.audit() == Some(&AgentError::AuditFailure))
    );
    assert!(fenced.load(Ordering::SeqCst));
    assert!(audit.owner.lock().unwrap().is_none());
    assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
    assert!(admission.wait().await.is_err());
    agent.close(actor()).await.unwrap();
}
