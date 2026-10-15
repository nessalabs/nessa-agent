//! The real shared adapter must not turn attachment evidence into an unbounded close.
use super::{
    finished, held_writer, held_writer_at, DurableExecutionAudit, ExecutionAudit, ExecutionOutcome,
    TestClock, WATCHDOG,
};
use crate::conversation_test_support::{Provider, ProviderFactory};
use nessa_sdk::application::agent_execution::{
    agents::{
        AdmissionEvidence, Agent, AgentError, AgentFuture, AttachmentPhase, AttachmentRequest,
    },
    executions::{
        AttachmentAuditCause, AttachmentAuditStage, ExecutionAuditRecord, ExecutionRequest,
        Submitter,
    },
    permissions::ActionContext,
    providers::SessionCloseRequest,
    sessions::{MessageCommitClock, MessageCommitSleep, SessionManager},
};
use nessa_sdk::domain::agent_execution::{
    executions::ExecutionId,
    prompts::{PromptText, UserMessage},
    sessions::AttachmentCause,
};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::{
    fs,
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

struct AuditClock(watch::Sender<Duration>);
impl AuditClock {
    fn new() -> Self {
        Self(watch::channel(Duration::ZERO).0)
    }
    fn advance(&self) {
        self.0.send_modify(|now| *now += Duration::from_secs(30));
    }
}
impl MessageCommitClock for AuditClock {
    fn now(&self) -> Duration {
        *self.0.borrow()
    }
    fn sleep_until(&self, deadline: Duration) -> MessageCommitSleep {
        let mut clock = self.0.subscribe();
        Box::pin(async move {
            while *clock.borrow() < deadline {
                clock.changed().await.unwrap();
            }
        })
    }
    fn unix_milliseconds(&self) -> u64 {
        1234
    }
}
struct ObservedAudit {
    real: Arc<DurableExecutionAudit>,
    records: Mutex<Vec<ExecutionAuditRecord>>,
    changed: watch::Sender<usize>,
}
impl ExecutionAudit for ObservedAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        self.records.lock().unwrap().push(record.clone());
        self.changed.send_modify(|version| *version += 1);
        self.real.record(record)
    }
}
impl ObservedAudit {
    async fn wait_for(&self, matches: impl Fn(&ExecutionAuditRecord) -> bool) {
        tokio::time::timeout(WATCHDOG, async {
            let mut changed = self.changed.subscribe();
            loop {
                if self.records.lock().unwrap().iter().any(&matches) {
                    return;
                }
                changed.changed().await.unwrap();
            }
        })
        .await
        .expect("the audit producer must attempt its evidence");
    }
}
#[derive(Clone, Copy)]
enum PendingAttachment {
    Authorized,
    Starting,
    Abandoned,
}

fn audit_failures(error: &AgentError) -> usize {
    match error {
        AgentError::AuditFailure => 1,
        AgentError::Closed => 0,
        AgentError::MultipleOperationFailures {
            first_error,
            subsequent_error,
        } => audit_failures(first_error) + audit_failures(subsequent_error),
        AgentError::OperationAndCleanupFailure {
            operation_error,
            cleanup_error,
        } => audit_failures(operation_error) + audit_failures(cleanup_error),
        other => panic!("unexpected typed failure: {other:?}"),
    }
}
async fn close_behind_stalled_write(stage: PendingAttachment) {
    let root = tempfile::tempdir().unwrap();
    let (writer, entered, release) = held_writer();
    let real = Arc::new(
        DurableExecutionAudit::with_writer(
            root.path().join("audit"),
            Arc::new(TestClock),
            writer.clone(),
        )
        .unwrap(),
    );
    let audit = Arc::new(ObservedAudit {
        real: real.clone(),
        records: Mutex::new(Vec::new()),
        changed: watch::channel(0).0,
    });
    let clock = Arc::new(AuditClock::new());
    let factory = Arc::new(ProviderFactory::default());
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()), clock.clone())
        .await
        .unwrap();
    let agent = Agent::prepare(
        Arc::new(Provider::new(factory.clone())),
        manager,
        audit.clone(),
    )
    .await
    .unwrap();
    let authorizer = ActionContext::new("owner", "credential", "authorize").unwrap();
    let closer = ActionContext::new("owner", "credential", "close").unwrap();
    let mut authorization = Some(
        agent
            .authorize_attachment(AttachmentRequest::CallerRequested(authorizer.clone()))
            .unwrap(),
    );
    let admission = tokio::spawn({
        let agent = agent.clone();
        let submitter = authorizer.clone();
        async move {
            agent
                .enqueue(
                    ExecutionRequest {
                        execution_id: ExecutionId::new("queued-before-open").unwrap(),
                        user_message: UserMessage::text_only(PromptText::new("message").unwrap()),
                        estimated_input_tokens: 1,
                        reserved_output_tokens: 1,
                    },
                    Submitter::owner(submitter),
                )
                .await
        }
    });
    tokio::time::timeout(WATCHDOG, entered)
        .await
        .unwrap()
        .unwrap();
    let attachment = match stage {
        PendingAttachment::Starting => {
            let waiting = agent
                .start_attachment(authorization.take().unwrap())
                .unwrap();
            audit
                .wait_for(|record| {
                    matches!(record, ExecutionAuditRecord::Attachment(record)
                if record.after() == AttachmentAuditStage::Starting)
                })
                .await;
            Some(waiting)
        }
        PendingAttachment::Abandoned => {
            drop(authorization.take());
            audit
                .wait_for(|record| {
                    matches!(record, ExecutionAuditRecord::Attachment(record)
                if record.cause() == AttachmentAuditCause::AuthorizationAbandoned)
                })
                .await;
            None
        }
        PendingAttachment::Authorized => None,
    };
    let close = tokio::spawn({
        let agent = agent.clone();
        let closer = closer.clone();
        async move { agent.close(closer).await }
    });
    if matches!(stage, PendingAttachment::Abandoned) {
        tokio::time::timeout(WATCHDOG, async {
            while agent.attachment_status().phase() != AttachmentPhase::Absent {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    } else {
        audit
            .wait_for(|record| {
                matches!(record, ExecutionAuditRecord::Attachment(record)
            if record.cause() == AttachmentAuditCause::Closed)
            })
            .await;
    }
    assert!(!close.is_finished());
    assert_eq!(factory.open_calls.load(Ordering::SeqCst), 0);
    assert!(factory.executions.lock().unwrap().is_empty());
    clock.advance();
    audit
        .wait_for(|record| matches!(record, ExecutionAuditRecord::QueueSettled(_)))
        .await;
    clock.advance();
    let admitted = tokio::time::timeout(WATCHDOG, admission)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(admitted.evidence(), AdmissionEvidence::Failed(failure)
        if failure.audit() == Some(&AgentError::AuditFailure))
    );
    let close = tokio::time::timeout(WATCHDOG, close)
        .await
        .expect("attachment evidence must not wait for the held physical write")
        .unwrap()
        .unwrap_err();
    assert!(audit_failures(&close) > 0);
    let receipt = tokio::time::timeout(WATCHDOG, admitted.wait())
        .await
        .unwrap()
        .unwrap_err();
    assert!(audit_failures(&receipt) > 0);
    if let Some(attachment) = attachment {
        let failure = tokio::time::timeout(WATCHDOG, attachment.wait())
            .await
            .unwrap()
            .unwrap_err();
        assert!(audit_failures(&failure) > 0);
    }
    {
        let records = audit.records.lock().unwrap();
        for record in records.iter().filter_map(|record| match record {
            ExecutionAuditRecord::Attachment(record) => Some(record),
            _ => None,
        }) {
            assert_eq!(record.generation(), 0);
            match record.cause() {
                AttachmentAuditCause::Closed => {
                    assert_eq!(record.actor(), Some(&closer));
                    assert_eq!(
                        record.before(),
                        if matches!(stage, PendingAttachment::Starting) {
                            AttachmentAuditStage::Starting
                        } else {
                            AttachmentAuditStage::Waiting
                        }
                    );
                    assert_eq!(record.after(), AttachmentAuditStage::Absent);
                }
                AttachmentAuditCause::Started(cause) => {
                    assert_eq!(cause, AttachmentCause::Initial);
                    assert_eq!(record.actor(), Some(&authorizer));
                }
                AttachmentAuditCause::AuthorizationAbandoned => {
                    assert_eq!(record.actor(), Some(&authorizer));
                    assert_eq!(record.before(), AttachmentAuditStage::Waiting);
                    assert_eq!(record.after(), AttachmentAuditStage::Absent);
                }
                other => panic!("unexpected attachment transition: {other:?}"),
            }
        }
    }
    assert_eq!(writer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(factory.open_calls.load(Ordering::SeqCst), 0);
    assert!(factory.executions.lock().unwrap().is_empty());
    release.0.release();
    tokio::time::timeout(WATCHDOG, real.record(finished(ExecutionOutcome::Completed)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(writer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(fs::read_dir(root.path().join("audit")).unwrap().count(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn attachment_close_evidence_times_out_behind_a_stalled_physical_audit() {
    close_behind_stalled_write(PendingAttachment::Authorized).await;
}
#[tokio::test(flavor = "current_thread")]
async fn starting_attachment_close_evidence_times_out_behind_a_stalled_physical_audit() {
    close_behind_stalled_write(PendingAttachment::Starting).await;
}
#[tokio::test(flavor = "current_thread")]
async fn abandoned_authorization_evidence_times_out_behind_a_stalled_physical_audit() {
    close_behind_stalled_write(PendingAttachment::Abandoned).await;
}

#[tokio::test(flavor = "current_thread")]
async fn publication_audit_deadline_releases_the_resource_guard_before_physical_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let (writer, entered, release) = held_writer_at(2);
    let real = Arc::new(
        DurableExecutionAudit::with_writer(
            root.path().join("audit"),
            Arc::new(TestClock),
            writer.clone(),
        )
        .unwrap(),
    );
    let audit = Arc::new(ObservedAudit {
        real: real.clone(),
        records: Mutex::new(Vec::new()),
        changed: watch::channel(0).0,
    });
    let clock = Arc::new(AuditClock::new());
    let factory = Arc::new(ProviderFactory::default());
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()), clock.clone())
        .await
        .unwrap();
    let agent = Agent::prepare(
        Arc::new(Provider::new(factory.clone())),
        manager,
        audit.clone(),
    )
    .await
    .unwrap();
    let authorizer = ActionContext::new("owner", "credential", "authorize-publication").unwrap();
    let closer = ActionContext::new("owner", "credential", "close-publication").unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(authorizer.clone()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    tokio::time::timeout(WATCHDOG, entered)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Starting);
    assert_eq!(factory.open_calls.load(Ordering::SeqCst), 1);
    let close = tokio::spawn({
        let agent = agent.clone();
        let closer = closer.clone();
        async move { agent.close(closer).await }
    });
    tokio::time::timeout(WATCHDOG, async {
        while agent.attachment_status().phase() != AttachmentPhase::Absent {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!close.is_finished());
    assert_eq!(factory.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(writer.calls.load(Ordering::SeqCst), 2);
    clock.advance();
    let close_failure = tokio::time::timeout(WATCHDOG, close)
        .await
        .expect("the bounded publication observer must release the resource guard")
        .unwrap()
        .unwrap_err();
    assert!(audit_failures(&close_failure) > 0);
    let start_failure = tokio::time::timeout(WATCHDOG, attachment.wait())
        .await
        .unwrap()
        .unwrap_err();
    assert!(audit_failures(&start_failure) > 0);
    assert_eq!(factory.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.close_requests.lock().unwrap(),
        vec![SessionCloseRequest::Explicit(closer)]
    );
    assert!(factory.executions.lock().unwrap().is_empty());
    assert_eq!(writer.calls.load(Ordering::SeqCst), 2);
    {
        let records = audit.records.lock().unwrap();
        let publication = records
            .iter()
            .find_map(|record| match record {
                ExecutionAuditRecord::Attachment(record)
                    if record.after() == AttachmentAuditStage::ContextPublished =>
                {
                    Some(record)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(publication.generation(), 0);
        assert_eq!(publication.actor(), Some(&authorizer));
        assert_eq!(publication.cause(), AttachmentAuditCause::Published);
        // Attached cleanup already owns the provider; no synthetic Started->Closed record is added.
        assert!(!records.iter().any(|record| matches!(record,
            ExecutionAuditRecord::Attachment(record) if record.cause() == AttachmentAuditCause::Closed)));
    }
    release.0.release();
    tokio::time::timeout(WATCHDOG, real.record(finished(ExecutionOutcome::Completed)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(writer.calls.load(Ordering::SeqCst), 3);
    assert_eq!(fs::read_dir(root.path().join("audit")).unwrap().count(), 3);
}
