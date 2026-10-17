use super::*;
use crate::conversation::application::{
    ConversationError, ReviewNoticeAudit, ReviewNoticeAuditCause, ReviewNoticeAuditRecord,
    ReviewNoticeClock as AuditClock, ReviewNoticeSleep,
};
use crate::conversation::infrastructure::{DurableReviewNoticeAudit, PermissionMemory};
use crate::conversation_test_support::{ObservedPolicy, RecordingReviewNoticeAudit};
use nessa_sdk::application::agent_execution::sessions::StorageError;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::{collections::VecDeque, fs, sync::Condvar};
use tokio::{runtime::Builder, sync::Notify, task::JoinHandle};

const TARGET: &str = "00000000-0000-4000-8000-00000000000a";

struct ReviewFixture {
    _directory: tempfile::TempDir,
    board: Arc<PermissionMemory>,
    state: ProductRouteState,
    authority: Arc<Authority>,
    repository: Arc<MemoryRepository>,
}

impl ReviewFixture {
    fn new(audit: Arc<dyn ReviewNoticeAudit>, clock: Arc<dyn AuditClock>) -> Self {
        let (directory, board) = open_rules();
        board.waiting(&ReviewNotice {
            conversation_id: TARGET.into(),
            execution_id: ExecutionId::new("turn").unwrap(),
            permission_id: PermissionId::new("permission").unwrap(),
            tool: "Bash".into(),
        });
        let (state, authority) = fixture(MembershipRole::Admin);
        let repository = repository_of(vec![owned_conversation(TARGET, "other")]);
        let state = state
            .with_permission_rules(board.clone())
            .with_passive_read(
                Arc::new(UnpairedBinding),
                repository.clone(),
                Arc::new(GrantedConversation(TARGET.into())),
                conversation_support::access_policy(),
            )
            .with_review_notice_auditor(Arc::new(ReviewNoticeAuditor::new(audit, clock)));
        Self {
            _directory: directory,
            board,
            state,
            authority,
            repository,
        }
    }

    async fn publish(
        &self,
        slots: Arc<Semaphore>,
    ) -> (
        JoinHandle<Result<(), ReviewNoticePublicationFailure>>,
        mpsc::Receiver<QueuedResponse>,
        watch::Sender<u64>,
    ) {
        let session = authenticate(&self.state).await;
        let (authority, updates) = watch::channel(session.auth_revision());
        let (send, receive) = mpsc::channel(1);
        let task = tokio::spawn(forward_review_notices(
            self.board.clone(),
            send,
            slots,
            self.state.clone(),
            session,
            updates,
        ));
        (task, receive, authority)
    }
}

/// Test port with one independent acknowledgement per explicitly gated record.
/// Capture and acknowledgement are separate observations.
struct GatedAudit {
    records: Mutex<Vec<Arc<ReviewNoticeAuditRecord>>>,
    releases: Mutex<VecDeque<oneshot::Receiver<Result<(), ReviewNoticeAuditFailure>>>>,
    entered: Semaphore,
    completed: Semaphore,
}

impl GatedAudit {
    fn one() -> (
        Arc<Self>,
        oneshot::Sender<Result<(), ReviewNoticeAuditFailure>>,
    ) {
        let (send, receive) = oneshot::channel();
        (
            Arc::new(Self {
                records: Mutex::new(Vec::new()),
                releases: Mutex::new(VecDeque::from([receive])),
                entered: Semaphore::new(0),
                completed: Semaphore::new(0),
            }),
            send,
        )
    }
    async fn captured(&self) {
        timeout(Duration::from_secs(2), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
    async fn finished(&self) {
        timeout(Duration::from_secs(2), self.completed.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}

impl ReviewNoticeAudit for GatedAudit {
    fn record(
        &self,
        record: Arc<ReviewNoticeAuditRecord>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send + '_>> {
        self.records.lock().unwrap().push(record);
        let release = self.releases.lock().unwrap().pop_front();
        self.entered.add_permits(1);
        Box::pin(async move {
            let result = match release {
                Some(release) => release.await.unwrap(),
                None => Ok(()),
            };
            self.completed.add_permits(1);
            result
        })
    }
}

#[derive(Default)]
struct ManualAuditClock {
    elapsed: Arc<AtomicU64>,
    changed: Arc<Notify>,
}

impl ManualAuditClock {
    fn advance(&self, millis: u64) {
        self.elapsed.fetch_add(millis, Ordering::SeqCst);
        self.changed.notify_waiters();
    }
}

impl AuditClock for ManualAuditClock {
    fn now(&self) -> u64 {
        self.elapsed.load(Ordering::SeqCst)
    }
    fn sleep_until(&self, deadline: u64) -> ReviewNoticeSleep {
        let elapsed = self.elapsed.clone();
        let changed = self.changed.clone();
        Box::pin(async move {
            loop {
                let notified = changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if elapsed.load(Ordering::SeqCst) >= deadline {
                    return;
                }
                notified.await;
            }
        })
    }
}

struct Answer(AccessDecision);
impl ConversationAccessPolicyEvaluator for Answer {
    fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
        assert_eq!(ask.action, AccessAction::ReviewNotice);
        self.0.clone()
    }
}

#[tokio::test]
async fn review_notice_policy_decisions_preserve_audit_evidence() {
    for decision in [
        AccessDecision::Allow {
            policies: vec!["precise.allow".into(), "second.allow".into()],
        },
        AccessDecision::Deny(AccessDenial::Forbidden {
            policies: vec!["precise.deny".into()],
        }),
        AccessDecision::Deny(AccessDenial::NotPermitted),
        AccessDecision::Deny(AccessDenial::Unavailable),
    ] {
        let audit = Arc::new(RecordingReviewNoticeAudit::default());
        let mut fixture =
            ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
        fixture.state.conversation_access = Some(Arc::new(Answer(decision.clone())));
        let expected = usize::from(matches!(decision, AccessDecision::Allow { .. }));
        let payload = notice_snapshot(
            fixture.board.clone(),
            fixture.state.clone(),
            authenticate(&fixture.state).await,
        )
        .await;
        assert_eq!(payload.reviews.len(), expected);
        let records = audit.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(
            record.decision(),
            &decision,
            "the complete Cedar decision must reach audit"
        );
        assert_eq!(record.conversation_id().to_string(), TARGET);
        assert_eq!(record.organization_id().as_str(), "organization");
        assert_eq!(record.owner_id().as_str(), "other");
        assert!(!record.deleted());
        assert_eq!(record.execution_id().as_str(), "turn");
        assert_eq!(record.permission_id().as_str(), "permission");
        assert_eq!(record.recipient().principal_id().as_str(), "principal");
        assert_eq!(record.recipient().credential_id().as_str(), "credential");
        assert_eq!(
            record.recipient().organization_id().as_str(),
            "organization"
        );
        assert_eq!(record.recipient().membership_id().as_str(), "membership");
        assert_eq!(record.recipient().audience_id().as_str(), "gateway");
        assert_eq!(record.membership(), MembershipRole::Admin);
        assert_eq!(record.authority_revision(), 1);
        assert_eq!(record.cause(), ReviewNoticeAuditCause::SnapshotRefresh);
        if expected == 1 {
            let wire = serde_json::to_string(&payload).unwrap();
            assert!(
                !wire.contains("precise.allow"),
                "policy IDs stay out of the UI notice"
            );
        }
    }
}

#[tokio::test]
async fn review_notice_publication_waits_for_audit_acknowledgement() {
    let (audit, acknowledge) = GatedAudit::one();
    let clock = Arc::new(ManualAuditClock::default());
    let fixture = ReviewFixture::new(audit.clone(), clock.clone());
    let (task, mut receive, authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
    audit.captured().await;
    assert!(
        receive.try_recv().is_err(),
        "capture alone must not disclose the review"
    );
    // Make both branches ready in the owner's next poll. A ready durable
    // acknowledgement wins its exact deadline, preserving the positive case.
    clock.advance(5_000);
    acknowledge.send(Ok(())).unwrap();
    let queued = timeout(Duration::from_secs(2), receive.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(queued.message, WireResponse::ReviewWaiting { payload, .. } if payload.reviews.len() == 1)
    );
    drop(authority);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn review_notice_reselects_after_authority_changes_during_audit() {
    for demote in [false, true] {
        let (audit, acknowledge) = GatedAudit::one();
        let fixture =
            ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
        let session = authenticate(&fixture.state).await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
        audit.captured().await;
        if demote {
            let mut current = snapshot(MembershipRole::Member, MembershipStatus::Active);
            current.revision = 2;
            *fixture.authority.snapshot.lock().unwrap() = current;
        }
        acknowledge.send(Ok(())).unwrap();
        let Message::Text(text) = timeout(Duration::from_secs(2), peer.output.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("review event expected")
        };
        let OutgoingMessage::Event(event) = serde_json::from_str(&text).unwrap() else {
            panic!("review event expected")
        };
        let payload: PermissionReviewWaiting = serde_json::from_value(event.payload).unwrap();
        assert_eq!(
            payload.reviews.len(),
            usize::from(!demote),
            "a selection demoted during audit must be reselected before disclosure"
        );
        {
            let records = audit.records.lock().unwrap();
            assert_eq!(records.len(), if demote { 2 } else { 1 });
            assert!(matches!(
                records[0].decision(),
                AccessDecision::Allow { .. }
            ));
            if demote {
                assert_eq!(records[1].authority_revision(), 2);
                assert!(matches!(
                    records[1].decision(),
                    AccessDecision::Deny(AccessDenial::NotPermitted)
                ));
            }
        }
        drop(peer);
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn review_notice_disabled_recipient_closes_after_audit() {
    let (audit, acknowledge) = GatedAudit::one();
    let fixture = ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
    let session = authenticate(&fixture.state).await;
    let (socket, mut peer) = test_socket(None);
    let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
    audit.captured().await;
    let mut current = snapshot(MembershipRole::Member, MembershipStatus::Disabled);
    current.revision = 2;
    *fixture.authority.snapshot.lock().unwrap() = current;
    acknowledge.send(Ok(())).unwrap();
    let Message::Close(Some(frame)) = timeout(Duration::from_secs(2), peer.output.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("disabled recipient must close before disclosure")
    };
    let close: SessionTermination = serde_json::from_str(&frame.reason).unwrap();
    assert_eq!(close.code, SessionCloseReason::AuthorizationLost);
    timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    let records = audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert!(
        matches!(records[0].decision(), AccessDecision::Allow { .. }),
        "the acknowledged prior selection keeps its original meaning"
    );
}

#[tokio::test]
async fn review_notice_reselects_after_shared_changes_during_audit() {
    for clear_board in [false, true] {
        let (audit, acknowledge) = GatedAudit::one();
        let fixture =
            ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
        let (task, mut frames, authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
        audit.captured().await;
        let next_acknowledgement = if clear_board {
            None
        } else {
            let (send, receive) = oneshot::channel();
            audit.releases.lock().unwrap().push_back(receive);
            Some(send)
        };
        if clear_board {
            fixture.board.clear(
                TARGET,
                &ExecutionId::new("turn").unwrap(),
                &PermissionId::new("permission").unwrap(),
            );
        } else {
            fixture.board.recheck_visibility();
        }
        acknowledge.send(Ok(())).unwrap();
        if let Some(next_acknowledgement) = next_acknowledgement {
            audit.captured().await;
            assert!(
                frames.try_recv().is_err(),
                "shared visibility changed during audit must be reselected before any frame"
            );
            next_acknowledgement.send(Ok(())).unwrap();
        }
        let queued = timeout(Duration::from_secs(2), frames.recv())
            .await
            .unwrap()
            .unwrap();
        let WireResponse::ReviewWaiting {
            payload,
            generation,
        } = queued.message
        else {
            panic!("review snapshot expected")
        };
        assert_eq!(
            payload.reviews.len(),
            usize::from(!clear_board),
            "a settled review must not be disclosed after a delayed audit"
        );
        assert_eq!(
            generation,
            fixture.board.current().0,
            "the admitted candidate must retain the current board generation"
        );
        assert_eq!(
            audit.records.lock().unwrap().len(),
            if clear_board { 1 } else { 2 },
            "shared changes during audit require reselection"
        );
        drop(queued._slot);
        drop(authority);
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn review_notice_mixed_authority_batch_is_reselected() {
    let (audit, acknowledge) = GatedAudit::one();
    let fixture = ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
    let other = "00000000-0000-4000-8000-00000000000b";
    let conversation = owned_conversation(other, "other");
    fixture
        .repository
        .records
        .lock()
        .unwrap()
        .insert(conversation.id().clone(), conversation);
    fixture.board.waiting(&ReviewNotice {
        conversation_id: other.into(),
        execution_id: ExecutionId::new("turn").unwrap(),
        permission_id: PermissionId::new("other-permission").unwrap(),
        tool: "Write".into(),
    });
    let (task, mut frames, authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
    audit.captured().await;
    let mut current = snapshot(MembershipRole::Member, MembershipStatus::Active);
    current.revision = 2;
    *fixture.authority.snapshot.lock().unwrap() = current;
    acknowledge.send(Ok(())).unwrap();
    let queued = timeout(Duration::from_secs(2), frames.recv())
        .await
        .unwrap()
        .unwrap();
    let WireResponse::ReviewWaiting { payload, .. } = queued.message else {
        panic!("review snapshot expected")
    };
    assert!(
        payload.reviews.is_empty(),
        "a candidate mixing current authority versions must be reselected"
    );
    {
        let records = audit.records.lock().unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(records[0].authority_revision(), 1);
        assert!(records[1..]
            .iter()
            .all(|record| record.authority_revision() == 2));
    }
    drop(queued._slot);
    drop(authority);
    task.await.unwrap().unwrap();
}

struct ChangingAudit {
    authority: Arc<Authority>,
    records: Mutex<Vec<Arc<ReviewNoticeAuditRecord>>>,
}
impl ReviewNoticeAudit for ChangingAudit {
    fn record(
        &self,
        record: Arc<ReviewNoticeAuditRecord>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send + '_>> {
        self.records.lock().unwrap().push(record);
        self.authority.snapshot.lock().unwrap().revision += 1;
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn review_notice_continuous_change_has_bounded_selection_attempts() {
    let mut fixture = ReviewFixture::new(
        Arc::new(RecordingReviewNoticeAudit::default()),
        Arc::new(MonotonicReviewNoticeClock::new()),
    );
    let audit = Arc::new(ChangingAudit {
        authority: fixture.authority.clone(),
        records: Mutex::new(Vec::new()),
    });
    fixture.state = fixture
        .state
        .with_review_notice_auditor(Arc::new(ReviewNoticeAuditor::new(
            audit.clone(),
            Arc::new(MonotonicReviewNoticeClock::new()),
        )));
    let session = authenticate(&fixture.state).await;
    let (socket, mut peer) = test_socket(None);
    let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
    let Message::Close(Some(frame)) = timeout(Duration::from_secs(2), peer.output.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("continuous selection changes must close without disclosure")
    };
    let close: SessionTermination = serde_json::from_str(&frame.reason).unwrap();
    assert_eq!(close.code, SessionCloseReason::TemporaryUnavailable);
    timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        audit.records.lock().unwrap().len(),
        3,
        "selection retries must remain bounded"
    );
}

#[tokio::test]
async fn review_notice_audit_failure_closes_without_disclosure() {
    for times_out in [false, true] {
        let (audit, acknowledge) = GatedAudit::one();
        let clock = Arc::new(ManualAuditClock::default());
        let fixture = ReviewFixture::new(audit.clone(), clock.clone());
        let session = authenticate(&fixture.state).await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
        audit.captured().await;
        assert!(
            peer.output.try_recv().is_err(),
            "unacknowledged audit must withhold every review event"
        );
        if times_out {
            clock.advance(5_000);
        } else {
            acknowledge
                .send(Err(ReviewNoticeAuditFailure::Unavailable))
                .unwrap();
        }
        let message = timeout(Duration::from_secs(2), peer.output.recv())
            .await
            .unwrap()
            .unwrap();
        let Message::Close(Some(frame)) = message else {
            panic!("audit failure must close before disclosure")
        };
        let close: SessionTermination = serde_json::from_str(&frame.reason).unwrap();
        assert_eq!(close.code, SessionCloseReason::TemporaryUnavailable);
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            peer.output.recv().await.is_none(),
            "cleanup must finish independently of audit"
        );
    }
}

#[tokio::test]
async fn review_notice_audit_survives_publisher_loss() {
    for result in [Ok(()), Err(ReviewNoticeAuditFailure::Unavailable)] {
        let (captured_log, _logging) = limit_log();
        let (audit, acknowledge) = GatedAudit::one();
        let fixture =
            ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
        let session = authenticate(&fixture.state).await;
        let (socket, peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
        audit.captured().await;
        // Closing the input makes the real connection owner abort its publisher.
        drop(peer);
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        acknowledge
            .send(result)
            .expect("publisher loss must not cancel its admitted audit");
        timeout(Duration::from_secs(2), audit.finished())
            .await
            .unwrap();
        assert_eq!(audit.records.lock().unwrap().len(), 1);
        assert_eq!(
            audit.records.lock().unwrap()[0]
                .conversation_id()
                .to_string(),
            TARGET
        );
        if result.is_err() {
            assert!(
                limit_text(&captured_log)
                    .contains("review-notice policy audit was not acknowledged"),
                "the delivery owner must report failure after its observer is gone"
            );
        }
    }
}

#[tokio::test]
async fn review_notice_audit_precedes_ui_backpressure() {
    let (audit, acknowledge) = GatedAudit::one();
    let fixture = ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
    let slots = Arc::new(Semaphore::new(1));
    let held = slots.clone().acquire_owned().await.unwrap();
    let (task, mut receive, authority) = fixture.publish(slots).await;
    timeout(Duration::from_secs(2), audit.captured())
        .await
        .expect("UI capacity must not prevent audit capture");
    acknowledge.send(Ok(())).unwrap();
    audit.finished().await;
    assert!(
        receive.try_recv().is_err(),
        "UI backpressure still holds publication"
    );
    drop(held);
    assert!(receive.recv().await.is_some());
    assert_eq!(audit.records.lock().unwrap().len(), 1);
    drop(authority);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn review_notice_audit_survives_wire_failure() {
    for result in [Ok(()), Err(ReviewNoticeAuditFailure::Unavailable)] {
        let (captured_log, _logging) = limit_log();
        let (audit, acknowledge) = GatedAudit::one();
        let fixture =
            ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
        let session = authenticate(&fixture.state).await;
        let (socket, peer) = test_socket(None);
        // Keep input alive so only physical response delivery fails.
        let TestPeer {
            input,
            output,
            mut writing,
        } = peer;
        drop(output);
        let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
        audit.captured().await;
        acknowledge.send(result).unwrap();
        timeout(Duration::from_secs(2), writing.recv())
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            audit.records.lock().unwrap().len(),
            1,
            "a failed write must retain the captured selection"
        );
        if result.is_err() {
            assert!(
                limit_text(&captured_log)
                    .contains("review-notice policy audit was not acknowledged"),
                "wire failure must not erase audit failure"
            );
        }
        drop(input);
    }
}

#[tokio::test]
async fn review_notice_missing_audit_refuses_before_policy() {
    let audit = Arc::new(RecordingReviewNoticeAudit::default());
    let mut fixture =
        ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
    let policy = Arc::new(ObservedPolicy::default());
    fixture.state.conversation_access = Some(policy.clone());
    fixture.state.review_auditor = None;
    let (task, mut receive, _authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
    assert_eq!(
        task.await.unwrap(),
        Err(ReviewNoticePublicationFailure::Audit(
            ReviewNoticeAuditFailure::Unavailable
        ))
    );
    assert!(receive.recv().await.is_none());
    assert_eq!(policy.asked(), 0);
    assert!(audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn review_notice_audit_admission_is_bounded_before_decision() {
    let audit = Arc::new(RecordingReviewNoticeAudit::default());
    let clock = Arc::new(ManualAuditClock::default());
    let mut fixture = ReviewFixture::new(audit.clone(), clock.clone());
    let policy = Arc::new(ObservedPolicy::default());
    fixture.state.conversation_access = Some(policy.clone());
    let auditor = fixture.state.review_auditor.as_ref().unwrap().clone();
    let mut admissions = Vec::new();
    for _ in 0..32 {
        admissions.push(auditor.admit().await.unwrap());
    }
    let (entered, mut metadata_read) = oneshot::channel();
    let (_release, gate) = oneshot::channel();
    *fixture.repository.metadata_load_gate.lock().unwrap() = Some((entered, gate));
    let session = authenticate(&fixture.state).await;
    let (_authority, updates) = watch::channel(session.auth_revision());
    let (send, mut receive) = mpsc::channel(1);
    let mut publication = Box::pin(forward_review_notices(
        fixture.board.clone(),
        send,
        Arc::new(Semaphore::new(1)),
        fixture.state,
        session,
        updates,
    ));
    assert!(futures_util::poll!(publication.as_mut()).is_pending());
    assert!(
        matches!(
            metadata_read.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ),
        "metadata must not run before audit capacity is admitted"
    );
    assert_eq!(policy.asked(), 0);
    clock.advance(5_000);
    assert_eq!(
        publication.await,
        Err(ReviewNoticePublicationFailure::Audit(
            ReviewNoticeAuditFailure::TimedOut
        ))
    );
    assert!(receive.recv().await.is_none());
    assert!(audit.records.lock().unwrap().is_empty());
    drop(admissions);
    // Bounded refusal does not poison future admissions.
    let _next = auditor.admit().await.unwrap();
}

#[tokio::test]
async fn review_notice_lost_publishers_retain_bounded_audit_admission() {
    let (audit, first_acknowledgement) = GatedAudit::one();
    let mut acknowledgements = vec![first_acknowledgement];
    for _ in 1..32 {
        let (send, receive) = oneshot::channel();
        audit.releases.lock().unwrap().push_back(receive);
        acknowledgements.push(send);
    }
    let mut fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
    let policy = Arc::new(ObservedPolicy::default());
    fixture.state.conversation_access = Some(policy.clone());
    let mut publishers = Vec::new();
    for _ in 0..32 {
        publishers.push(fixture.publish(Arc::new(Semaphore::new(1))).await);
    }
    for _ in 0..32 {
        audit.captured().await;
    }
    for (publisher, _, _) in publishers {
        publisher.abort();
        assert!(publisher.await.unwrap_err().is_cancelled());
    }
    let session = authenticate(&fixture.state).await;
    let (authority, updates) = watch::channel(session.auth_revision());
    let (send, mut receive) = mpsc::channel(1);
    let mut publication = Box::pin(forward_review_notices(
        fixture.board.clone(),
        send,
        Arc::new(Semaphore::new(1)),
        fixture.state.clone(),
        session,
        updates,
    ));
    assert!(futures_util::poll!(publication.as_mut()).is_pending());
    assert_eq!(
        policy.asked(),
        32,
        "lost publishers must not free audit ownership capacity early"
    );
    assert_eq!(audit.records.lock().unwrap().len(), 32);
    acknowledgements.remove(0).send(Ok(())).unwrap();
    audit.finished().await;
    let queued = drive_review_publication(publication.as_mut(), &mut receive).await;
    assert_eq!(policy.asked(), 33);
    assert_eq!(audit.records.lock().unwrap().len(), 33);
    audit.finished().await;
    drop(queued);
    drop(authority);
    publication.await.unwrap();
    for acknowledgement in acknowledgements {
        acknowledgement
            .send(Ok(()))
            .expect("each lost publisher keeps its admitted audit alive");
    }
    for _ in 1..32 {
        audit.finished().await;
    }
}

struct PanickingAuditClock {
    calls: AtomicU64,
    fail_on: u64,
}
impl AuditClock for PanickingAuditClock {
    fn now(&self) -> u64 {
        assert_ne!(
            self.calls.fetch_add(1, Ordering::SeqCst),
            self.fail_on,
            "audit clock fault"
        );
        0
    }
    fn sleep_until(&self, _: u64) -> ReviewNoticeSleep {
        Box::pin(std::future::pending())
    }
}

#[tokio::test]
async fn review_notice_audit_clock_panic_closes_without_disclosure() {
    let clock_fixture = |fail_on| {
        let audit = Arc::new(RecordingReviewNoticeAudit::default());
        let mut fixture = ReviewFixture::new(
            audit.clone(),
            Arc::new(PanickingAuditClock {
                calls: AtomicU64::new(0),
                fail_on,
            }),
        );
        let policy = Arc::new(ObservedPolicy::default());
        fixture.state.conversation_access = Some(policy.clone());
        (fixture, audit, policy)
    };
    for fail_on in [0, 1] {
        let (typed_fixture, typed_audit, typed_policy) = clock_fixture(fail_on);
        let typed_session = authenticate(&typed_fixture.state).await;
        let (_authority, updates) = watch::channel(typed_session.auth_revision());
        let (send, mut receive) = mpsc::channel(1);
        assert_eq!(
            forward_review_notices(
                typed_fixture.board.clone(),
                send,
                Arc::new(Semaphore::new(1)),
                typed_fixture.state,
                typed_session,
                updates,
            )
            .await,
            Err(ReviewNoticePublicationFailure::Audit(
                ReviewNoticeAuditFailure::Panicked
            )),
            "the original clock panic must remain typed, before or after policy"
        );
        assert!(receive.recv().await.is_none());
        assert_eq!(typed_policy.asked(), usize::try_from(fail_on).unwrap());
        assert!(typed_audit.records.lock().unwrap().is_empty());

        let (fixture, audit, policy) = clock_fixture(fail_on);
        let session = authenticate(&fixture.state).await;
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
        let Message::Close(Some(frame)) = timeout(Duration::from_secs(2), peer.output.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("audit clock failure must close before disclosure")
        };
        let close: SessionTermination = serde_json::from_str(&frame.reason).unwrap();
        assert_eq!(close.code, SessionCloseReason::TemporaryUnavailable);
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(policy.asked(), usize::try_from(fail_on).unwrap());
        assert!(audit.records.lock().unwrap().is_empty());
    }
}

struct PanickingAudit(bool);
impl ReviewNoticeAudit for PanickingAudit {
    fn record(
        &self,
        _: Arc<ReviewNoticeAuditRecord>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send + '_>> {
        if self.0 {
            panic!("sink invocation fault")
        }
        Box::pin(async { panic!("sink polling fault") })
    }
}

#[tokio::test]
async fn review_notice_audit_sink_panic_closes_without_disclosure() {
    for invocation in [false, true] {
        let fixture = ReviewFixture::new(
            Arc::new(PanickingAudit(invocation)),
            Arc::new(MonotonicReviewNoticeClock::new()),
        );
        let session = authenticate(&fixture.state).await;
        let (_authority, updates) = watch::channel(session.auth_revision());
        let (send, mut receive) = mpsc::channel(1);
        assert_eq!(
            forward_review_notices(
                fixture.board.clone(),
                send,
                Arc::new(Semaphore::new(1)),
                fixture.state.clone(),
                session.clone(),
                updates,
            )
            .await,
            Err(ReviewNoticePublicationFailure::Audit(
                ReviewNoticeAuditFailure::Panicked
            )),
            "the audit owner must contain and report the original sink panic"
        );
        assert!(receive.recv().await.is_none());
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
        let Message::Close(Some(frame)) = timeout(Duration::from_secs(2), peer.output.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("sink panic must close before disclosure")
        };
        let close: SessionTermination = serde_json::from_str(&frame.reason).unwrap();
        assert_eq!(close.code, SessionCloseReason::TemporaryUnavailable);
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn review_notice_refused_facts_do_not_fabricate_policy_audit() {
    for refusal in 0..4 {
        let audit = Arc::new(RecordingReviewNoticeAudit::default());
        let mut fixture =
            ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
        let policy = Arc::new(ObservedPolicy::default());
        fixture.state.conversation_access = Some(policy.clone());
        let session = authenticate(&fixture.state).await;
        match refusal {
            0 => {
                fixture.repository.records.lock().unwrap().clear();
            }
            1 => {
                *fixture.repository.mode_loaded_reply.lock().unwrap() = Some(owned_conversation(
                    "00000000-0000-4000-8000-00000000000b",
                    "other",
                ));
            }
            2 => {
                *fixture.authority.snapshot.lock().unwrap() =
                    snapshot(MembershipRole::Admin, MembershipStatus::Disabled);
            }
            3 => {
                fixture.state.access = Arc::new(UnavailableAccess);
            }
            _ => unreachable!(),
        }
        if refusal == 0 {
            let payload = notice_snapshot(fixture.board, fixture.state, session).await;
            assert!(payload.reviews.is_empty());
        } else {
            let (_authority, updates) = watch::channel(session.auth_revision());
            let (send, mut receive) = mpsc::channel(1);
            let result = forward_review_notices(
                fixture.board,
                send,
                Arc::new(Semaphore::new(1)),
                fixture.state,
                session,
                updates,
            )
            .await;
            assert_eq!(
                result,
                Err(ReviewNoticePublicationFailure::Audit(
                    ReviewNoticeAuditFailure::Unavailable
                ))
            );
            assert!(receive.recv().await.is_none());
        }
        assert_eq!(
            policy.asked(),
            0,
            "untrusted facts must be refused before Cedar"
        );
        assert!(
            audit.records.lock().unwrap().is_empty(),
            "no policy answer must be fabricated for refused facts"
        );
    }
}

struct PhysicalClock {
    calls: AtomicU64,
    entered: Semaphore,
    released: Mutex<bool>,
    release: Condvar,
}

impl PhysicalClock {
    fn held() -> Self {
        Self {
            calls: AtomicU64::new(0),
            entered: Semaphore::new(0),
            released: Mutex::new(false),
            release: Condvar::new(),
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.release.notify_all();
    }
}

impl Clock for PhysicalClock {
    fn unix_milliseconds(&self) -> u64 {
        let sequence = self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.release.wait(released).unwrap();
        }
        1_000 + sequence
    }
}

struct ReleasePhysical(Arc<PhysicalClock>);
impl Drop for ReleasePhysical {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct DroppedObserver(Arc<Semaphore>);
impl Drop for DroppedObserver {
    fn drop(&mut self) {
        self.0.add_permits(1);
    }
}

/// Observes cancellation of the real adapter's async result, without changing
/// its physical worker or adding a production-only test path.
struct ObservedAdapter {
    audit: DurableReviewNoticeAudit,
    started: Semaphore,
    dropped: Arc<Semaphore>,
}
impl ReviewNoticeAudit for ObservedAdapter {
    fn record(
        &self,
        record: Arc<ReviewNoticeAuditRecord>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send + '_>> {
        let operation = self.audit.record(record);
        Box::pin(async move {
            let _observer = DroppedObserver(self.dropped.clone());
            self.started.add_permits(1);
            operation.await
        })
    }
}

fn stored(directory: &Path) -> Vec<Value> {
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .collect()
}

#[tokio::test]
async fn review_notice_durable_audit_preserves_typed_decisions() {
    for (decision, expected) in [
        (
            AccessDecision::Allow {
                policies: vec!["allow.actual".into()],
            },
            json!({"kind":"allow", "policies":["allow.actual"]}),
        ),
        (
            AccessDecision::Deny(AccessDenial::Forbidden {
                policies: vec!["deny.actual".into()],
            }),
            json!({"kind":"deny", "reason":"forbidden", "policies":["deny.actual"]}),
        ),
        (
            AccessDecision::Deny(AccessDenial::NotPermitted),
            json!({"kind":"deny", "reason":"not_permitted", "policies":[]}),
        ),
        (
            AccessDecision::Deny(AccessDenial::Unavailable),
            json!({"kind":"unavailable", "policies":[]}),
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("review-audit");
        let audit = Arc::new(
            DurableReviewNoticeAudit::new(path.clone(), Arc::new(FixedClock), Uuid::from_u128(3))
                .unwrap(),
        );
        let mut fixture = ReviewFixture::new(audit, Arc::new(MonotonicReviewNoticeClock::new()));
        fixture.state.conversation_access = Some(Arc::new(Answer(decision)));
        notice_snapshot(
            fixture.board.clone(),
            fixture.state.clone(),
            authenticate(&fixture.state).await,
        )
        .await;
        let records = stored(&path);
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0]["decision"], expected,
            "durable storage must preserve the full typed policy answer"
        );
        assert_eq!(
            records[0]["target"],
            json!({"conversationId":TARGET, "organizationId":"organization", "ownerId":"other", "deleted":false, "executionId":"turn", "permissionId":"permission"})
        );
        assert_eq!(
            records[0]["recipient"],
            json!({"organizationId":"organization", "principalId":"principal", "membershipId":"membership", "credentialId":"credential", "audienceId":"gateway", "membershipRole":"admin", "authorityRevision":1})
        );
        assert_eq!(
            records[0]["initiator"],
            json!({"kind":"system", "cause":"snapshot_refresh"})
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_notice_durable_audit_retains_physical_write() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("review-audit");
    let physical_clock = Arc::new(PhysicalClock::held());
    let _release_on_exit = ReleasePhysical(physical_clock.clone());
    let audit = Arc::new(ObservedAdapter {
        audit: DurableReviewNoticeAudit::new(
            path.clone(),
            physical_clock.clone(),
            Uuid::from_u128(1),
        )
        .unwrap(),
        started: Semaphore::new(0),
        dropped: Arc::new(Semaphore::new(0)),
    });
    let observer_clock = Arc::new(ManualAuditClock::default());
    let mut fixture = ReviewFixture::new(audit.clone(), observer_clock.clone());
    let policy = Arc::new(ObservedPolicy::default());
    fixture.state.conversation_access = Some(policy.clone());
    let (first, mut first_frames, _authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
    timeout(Duration::from_secs(2), physical_clock.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    audit.started.acquire().await.unwrap().forget();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(first_frames.recv().await.is_none());
    // The independent audit observer times out too. Its physical write remains
    // blocked inside the real adapter and still owns the instance's one slot.
    observer_clock.advance(5_000);
    timeout(Duration::from_secs(2), audit.dropped.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let (second, mut second_frames, authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
    timeout(Duration::from_secs(2), async {
        while policy.asked() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    timeout(Duration::from_secs(2), audit.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(
        physical_clock.calls.load(Ordering::SeqCst),
        1,
        "a dropped observer must not release the physical slot"
    );
    assert!(stored(&path).is_empty());
    assert!(second_frames.try_recv().is_err());
    physical_clock.release();
    timeout(Duration::from_secs(2), second_frames.recv())
        .await
        .unwrap()
        .unwrap();
    drop(authority);
    second.await.unwrap().unwrap();
    let records = stored(&path);
    assert_eq!(
        records.len(),
        2,
        "the lost publisher's record must still commit"
    );
    assert_eq!(records[0]["observedAtMs"], 1000);
    assert_eq!(records[1]["observedAtMs"], 1001);
    for (sequence, record) in records.iter().enumerate() {
        assert_eq!(
            record["recordId"],
            format!("review-notice-00000000-0000-0000-0000-000000000001-{sequence}")
        );
        assert_eq!(record["kind"], "review_notice_selection");
        assert_eq!(record["action"], "review.notice");
        assert_eq!(record["target"]["conversationId"], TARGET);
        assert_eq!(record["target"]["executionId"], "turn");
        assert_eq!(record["target"]["permissionId"], "permission");
        assert_eq!(record["recipient"]["principalId"], "principal");
        assert_eq!(record["recipient"]["membershipRole"], "admin");
        assert_eq!(
            record["initiator"],
            json!({"kind":"system", "cause":"snapshot_refresh"})
        );
        assert_eq!(record["decision"]["kind"], "allow");
        assert!(!record["decision"]["policies"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(!record.to_string().contains("secret"));
    }
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for entry in fs::read_dir(&path).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[tokio::test]
async fn review_notice_durable_audit_failure_prevents_publication_and_preserves_records() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("review-audit");
    let held = directory.path().join("held-audit");
    let audit = Arc::new(
        DurableReviewNoticeAudit::new(path.clone(), Arc::new(FixedClock), Uuid::from_u128(2))
            .unwrap(),
    );
    let fixture = ReviewFixture::new(audit, Arc::new(MonotonicReviewNoticeClock::new()));
    let (first, mut frames, authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
    timeout(Duration::from_secs(2), frames.recv())
        .await
        .unwrap()
        .unwrap();
    drop(authority);
    first.await.unwrap().unwrap();
    let before = stored(&path);
    fs::rename(&path, &held).unwrap();
    fs::write(&path, b"refuse a non-directory destination").unwrap();
    let (failed, mut frames, _authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
    assert_eq!(
        timeout(Duration::from_secs(2), failed)
            .await
            .unwrap()
            .unwrap(),
        Err(ReviewNoticePublicationFailure::Audit(
            ReviewNoticeAuditFailure::Unavailable
        ))
    );
    assert!(
        frames.recv().await.is_none(),
        "failed physical persistence must withhold the snapshot"
    );
    assert_eq!(
        stored(&held),
        before,
        "failure must preserve the original evidence"
    );
    fs::remove_file(&path).unwrap();
    fs::rename(&held, &path).unwrap();
    let (recovered, mut frames, authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
    timeout(Duration::from_secs(2), frames.recv())
        .await
        .unwrap()
        .unwrap();
    drop(authority);
    recovered.await.unwrap().unwrap();
    let records = stored(&path);
    assert_eq!(records.len(), 2);
    assert_eq!(&records[..1], &before);
    assert!(
        records[1]["recordId"].as_str().unwrap().ends_with("-2"),
        "a failed attempt must not reuse its storage identity"
    );
}

#[tokio::test]
async fn review_notice_audit_failure_with_full_control_lane_ends_connection() {
    let (captured, _logging) = limit_log();
    let (audit, acknowledge) = GatedAudit::one();
    let fixture = ReviewFixture::new(audit.clone(), Arc::new(MonotonicReviewNoticeClock::new()));
    let session = authenticate(&fixture.state).await;
    let (release, gate) = oneshot::channel();
    let (socket, mut peer) = test_socket(Some(gate));
    let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
    audit.captured().await;
    // Hold an ordinary physical send, leaving every control slot available.
    admit(&peer, "server.health", "held-ordinary").await;
    timeout(Duration::from_secs(2), peer.writing.recv())
        .await
        .unwrap()
        .unwrap();
    for index in 0..fixture.state.limits.control_lane() {
        admit(&peer, "conversation.cancel", &format!("control-{index}")).await;
    }
    acknowledge
        .send(Err(ReviewNoticeAuditFailure::Unavailable))
        .unwrap();
    // This event is emitted only after the real close enqueue observes Full.
    // Requiring it proves the regression actually filled the control lane.
    timeout(Duration::from_secs(2), async {
        while !limit_text(&captured).contains("socket.control_lane") {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fatal audit close must encounter the full control lane");
    assert!(
        !peer.input.is_closed(),
        "the peer input remains live during the held physical write"
    );
    release.send(()).unwrap();
    timeout(Duration::from_secs(2), task)
        .await
        .expect("the connection owner must terminate despite a rejected close enqueue")
        .unwrap();
    assert!(
        peer.input.is_closed(),
        "the owner must stop receiving while the peer still owns its sender"
    );
    while peer.output.recv().await.is_some() {}
}

struct PanickingReviewPolicy;
impl ConversationAccessPolicyEvaluator for PanickingReviewPolicy {
    fn evaluate(&self, _: &AccessAsk<'_>) -> AccessDecision {
        panic!("publisher policy fault")
    }
}

#[tokio::test]
async fn review_notice_publisher_panic_ends_connection() {
    let mut fixture = ReviewFixture::new(
        Arc::new(RecordingReviewNoticeAudit::default()),
        Arc::new(MonotonicReviewNoticeClock::new()),
    );
    fixture.state.conversation_access = Some(Arc::new(PanickingReviewPolicy));
    let session = authenticate(&fixture.state).await;
    let (socket, mut peer) = test_socket(None);
    let task = tokio::spawn(run_authenticated(socket, fixture.state, session));
    let Message::Close(Some(frame)) = timeout(Duration::from_secs(2), peer.output.recv())
        .await
        .expect("unexpected publisher completion must close the live connection")
        .unwrap()
    else {
        panic!("publisher failure must close before any review is published")
    };
    let close: SessionTermination = serde_json::from_str(&frame.reason).unwrap();
    assert_eq!(close.code, SessionCloseReason::TemporaryUnavailable);
    timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(peer.input.is_closed());
}

#[tokio::test]
async fn review_notice_shutdown_distinguishes_predecision_cancellation_from_transferred_work() {
    let audit = Arc::new(RecordingReviewNoticeAudit::default());
    let mut fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
    let policy = Arc::new(ObservedPolicy::default());
    fixture.state.conversation_access = Some(policy.clone());
    let auditor = fixture.state.review_auditor.as_ref().unwrap().clone();
    let (entered, metadata_read) = oneshot::channel();
    let (_release, gate) = oneshot::channel();
    *fixture.repository.metadata_load_gate.lock().unwrap() = Some((entered, gate));
    let (publisher, _frames, _authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
    timeout(Duration::from_secs(2), metadata_read)
        .await
        .unwrap()
        .unwrap();
    auditor.close();
    assert!(auditor.admit().await.is_err());
    let mut drain = Box::pin(auditor.drain());
    assert!(futures_util::poll!(drain.as_mut()).is_pending());
    publisher.abort();
    assert!(publisher.await.unwrap_err().is_cancelled());
    assert_eq!(
        timeout(Duration::from_secs(2), drain).await.unwrap(),
        Ok(())
    );
    assert_eq!(policy.asked(), 0);
    assert!(audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn review_notice_shutdown_joins_transferred_decisions_after_publisher_loss() {
    for result in [Ok(()), Err(ReviewNoticeAuditFailure::Unavailable)] {
        let (audit, acknowledge) = GatedAudit::one();
        let fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
        let auditor = fixture.state.review_auditor.as_ref().unwrap().clone();
        let (publisher, _frames, _authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
        audit.captured().await;
        publisher.abort();
        assert!(publisher.await.unwrap_err().is_cancelled());
        auditor.close();
        let mut drain = Box::pin(auditor.drain());
        assert!(futures_util::poll!(drain.as_mut()).is_pending());
        acknowledge.send(result).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), drain).await.unwrap(),
            result
        );
        assert_eq!(auditor.drain().await, result);
    }
}

#[tokio::test]
async fn review_notice_shutdown_retains_lost_transferred_task() {
    let audit = Arc::new(RecordingReviewNoticeAudit::default());
    let fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
    notice_snapshot(
        fixture.board.clone(),
        fixture.state.clone(),
        authenticate(&fixture.state).await,
    )
    .await;
    let record = audit.records.lock().unwrap()[0].clone();
    let auditor = Arc::new(ReviewNoticeAuditor::new(
        audit,
        Arc::new(ManualAuditClock::default()),
    ));
    let admitted = auditor.admit().await.unwrap();
    // A runtime that is never polled drops the transferred owner before its
    // first instruction. There is no task left that could publish its result.
    let observer = tokio::task::spawn_blocking(move || {
        let runtime = Builder::new_current_thread().build().unwrap();
        let entered = runtime.enter();
        let observer = admitted.record(record);
        drop(entered);
        drop(runtime);
        observer
    })
    .await
    .unwrap();
    assert_eq!(observer.await, Err(ReviewNoticeAuditFailure::Unavailable));
    auditor.close();
    assert_eq!(
        auditor.drain().await,
        Err(ReviewNoticeAuditFailure::Unavailable)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_notice_shutdown_joins_physical_write_and_queued_selection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("review-audit");
    let physical_clock = Arc::new(PhysicalClock::held());
    let _release = ReleasePhysical(physical_clock.clone());
    let audit = Arc::new(ObservedAdapter {
        audit: DurableReviewNoticeAudit::new(
            path.clone(),
            physical_clock.clone(),
            Uuid::from_u128(5),
        )
        .unwrap(),
        started: Semaphore::new(0),
        dropped: Arc::new(Semaphore::new(0)),
    });
    let fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
    let auditor = fixture.state.review_auditor.as_ref().unwrap().clone();
    let (first, _first_frames, _first_authority) =
        fixture.publish(Arc::new(Semaphore::new(1))).await;
    timeout(Duration::from_secs(2), physical_clock.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    audit.started.acquire().await.unwrap().forget();
    let (second, _second_frames, _second_authority) =
        fixture.publish(Arc::new(Semaphore::new(1))).await;
    timeout(Duration::from_secs(2), audit.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(physical_clock.calls.load(Ordering::SeqCst), 1);
    first.abort();
    second.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(second.await.unwrap_err().is_cancelled());
    auditor.close();
    let mut drain = Box::pin(auditor.drain());
    assert!(futures_util::poll!(drain.as_mut()).is_pending());
    assert!(stored(&path).is_empty());
    physical_clock.release();
    // Scheduler and filesystem hang watchdog for the two durable writes;
    // the injected product deadline clock stays frozen in this test.
    const DURABLE_DRAIN_HANG: Duration = Duration::from_secs(60);
    assert_eq!(
        timeout(DURABLE_DRAIN_HANG, drain)
            .await
            .expect("released physical writes and their queued selection must finish draining"),
        Ok(())
    );
    assert_eq!(stored(&path).len(), 2);
}

#[tokio::test]
async fn review_notice_shutdown_retains_first_delivery_failure() {
    for first_failure in [
        ReviewNoticeAuditFailure::Unavailable,
        ReviewNoticeAuditFailure::TimedOut,
    ] {
        let (audit, acknowledge) = GatedAudit::one();
        let fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
        let auditor = fixture.state.review_auditor.as_ref().unwrap().clone();
        let (first, _frames, _authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
        audit.captured().await;
        acknowledge.send(Err(first_failure)).unwrap();
        assert_eq!(
            first.await.unwrap(),
            Err(ReviewNoticePublicationFailure::Audit(first_failure))
        );
        let (second, mut frames, authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
        assert!(timeout(Duration::from_secs(2), frames.recv())
            .await
            .unwrap()
            .is_some());
        drop(authority);
        second.await.unwrap().unwrap();
        auditor.close();
        assert_eq!(auditor.drain().await, Err(first_failure));
        assert_eq!(auditor.expire(), first_failure);
    }
    for invocation in [true, false] {
        let fixture = ReviewFixture::new(
            Arc::new(PanickingAudit(invocation)),
            Arc::new(ManualAuditClock::default()),
        );
        let auditor = fixture.state.review_auditor.as_ref().unwrap().clone();
        let (publisher, _frames, _authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
        assert_eq!(
            publisher.await.unwrap(),
            Err(ReviewNoticePublicationFailure::Audit(
                ReviewNoticeAuditFailure::Panicked
            ))
        );
        auditor.close();
        assert_eq!(
            auditor.drain().await,
            Err(ReviewNoticeAuditFailure::Panicked)
        );
        assert_eq!(auditor.expire(), ReviewNoticeAuditFailure::Panicked);
    }
}

#[tokio::test]
async fn review_notice_shutdown_deadline_retains_first_failure() {
    for earlier_failure in [None, Some(ReviewNoticeAuditFailure::Unavailable)] {
        let (audit, acknowledge) = GatedAudit::one();
        let fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
        let auditor = fixture.state.review_auditor.as_ref().unwrap().clone();
        let (first, _frames, _authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
        audit.captured().await;
        let (last, last_acknowledgement) = if let Some(failure) = earlier_failure {
            acknowledge.send(Err(failure)).unwrap();
            assert_eq!(
                first.await.unwrap(),
                Err(ReviewNoticePublicationFailure::Audit(failure))
            );
            let (send, receive) = oneshot::channel();
            audit.releases.lock().unwrap().push_back(receive);
            let (last, _frames, _authority) = fixture.publish(Arc::new(Semaphore::new(1))).await;
            audit.captured().await;
            (last, send)
        } else {
            (first, acknowledge)
        };
        auditor.close();
        let mut drain = Box::pin(auditor.drain());
        assert!(futures_util::poll!(drain.as_mut()).is_pending());
        let expected = earlier_failure.unwrap_or(ReviewNoticeAuditFailure::TimedOut);
        assert_eq!(auditor.expire(), expected);
        last_acknowledgement
            .send(Err(ReviewNoticeAuditFailure::Panicked))
            .unwrap();
        assert_eq!(
            last.await.unwrap(),
            Err(ReviewNoticePublicationFailure::Audit(
                ReviewNoticeAuditFailure::Panicked
            ))
        );
        assert_eq!(
            timeout(Duration::from_secs(2), drain).await.unwrap(),
            Err(expected)
        );
        assert_eq!(auditor.drain().await, Err(expected));
    }
}

#[tokio::test]
async fn review_notice_shutdown_wakes_capacity_waiters_and_joins_admissions() {
    let fixture = ReviewFixture::new(
        Arc::new(RecordingReviewNoticeAudit::default()),
        Arc::new(ManualAuditClock::default()),
    );
    let auditor = fixture.state.review_auditor.as_ref().unwrap();
    let mut admissions = Vec::new();
    for _ in 0..32 {
        admissions.push(auditor.admit().await.unwrap());
    }
    let mut waiting = Box::pin(auditor.admit());
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    auditor.close();
    assert!(matches!(
        timeout(Duration::from_secs(2), waiting)
            .await
            .expect("closing admission must wake a capacity waiter"),
        Err(ReviewNoticeAuditFailure::Unavailable)
    ));
    let mut drain = Box::pin(auditor.drain());
    assert!(futures_util::poll!(drain.as_mut()).is_pending());
    drop(admissions);
    assert_eq!(
        timeout(Duration::from_secs(2), drain).await.unwrap(),
        Ok(())
    );
}

#[tokio::test]
async fn review_notice_metadata_read_failure_closes_before_false_empty_snapshot() {
    let audit = Arc::new(RecordingReviewNoticeAudit::default());
    let mut fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
    let policy = Arc::new(ObservedPolicy::default());
    fixture.state.conversation_access = Some(policy.clone());
    *fixture.repository.metadata_load_failure.lock().unwrap() = Some(ConversationError::Storage(
        StorageError::Io("injected metadata read failure".into()),
    ));
    let session = authenticate(&fixture.state).await;
    let (socket, mut peer) = test_socket(None);
    let task = tokio::spawn(run_authenticated(socket, fixture.state.clone(), session));
    let message = timeout(Duration::from_secs(2), peer.output.recv())
        .await
        .unwrap()
        .unwrap();
    let Message::Close(Some(frame)) = message else {
        panic!("metadata failure must close before a false empty replacement: {message:?}")
    };
    let close: SessionTermination = serde_json::from_str(&frame.reason).unwrap();
    assert_eq!(close.code, SessionCloseReason::TemporaryUnavailable);
    timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(peer.output.recv().await.is_none());
    assert_eq!(policy.asked(), 0);
    assert!(audit.records.lock().unwrap().is_empty());
    // No board, share or authentication revision changes: only the read recovers.
    let recovered = notice_snapshot(
        fixture.board.clone(),
        fixture.state.clone(),
        authenticate(&fixture.state).await,
    )
    .await;
    assert_eq!(recovered.reviews.len(), 1);
    assert_eq!(recovered.reviews[0].conversation_id, TARGET);
    assert_eq!(policy.asked(), 1);
    assert_eq!(audit.records.lock().unwrap().len(), 1);
}

struct FailNextCurrentRead {
    authority: Arc<Authority>,
    fail: AtomicBool,
}

impl AccessReader for FailNextCurrentRead {
    fn read<'a>(&'a self, credential: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        if self.fail.swap(false, Ordering::SeqCst) {
            Box::pin(async { Err(AccessError::Unavailable) })
        } else {
            self.authority.read(credential)
        }
    }
}

#[tokio::test]
async fn review_notice_current_read_failure_prevents_false_empty_snapshot() {
    let audit = Arc::new(RecordingReviewNoticeAudit::default());
    let mut fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
    let policy = Arc::new(ObservedPolicy::default());
    fixture.state.conversation_access = Some(policy.clone());
    let reader = Arc::new(FailNextCurrentRead {
        authority: fixture.authority.clone(),
        fail: AtomicBool::new(false),
    });
    fixture.state.access = reader.clone();
    let session = authenticate(&fixture.state).await;
    reader.fail.store(true, Ordering::SeqCst);
    let (_authority, updates) = watch::channel(session.auth_revision());
    let (send, mut receive) = mpsc::channel(1);
    let task = tokio::spawn(forward_review_notices(
        fixture.board.clone(),
        send,
        Arc::new(Semaphore::new(1)),
        fixture.state.clone(),
        session,
        updates,
    ));
    let queued = timeout(Duration::from_secs(2), receive.recv())
        .await
        .unwrap();
    assert!(
        queued.is_none(),
        "current-read failure published a false empty replacement"
    );
    assert_eq!(
        task.await.unwrap(),
        Err(ReviewNoticePublicationFailure::Audit(
            ReviewNoticeAuditFailure::Unavailable
        ))
    );
    assert_eq!(policy.asked(), 0);
    assert!(audit.records.lock().unwrap().is_empty());
    let recovered = notice_snapshot(
        fixture.board.clone(),
        fixture.state.clone(),
        authenticate(&fixture.state).await,
    )
    .await;
    assert_eq!(recovered.reviews.len(), 1);
    assert_eq!(policy.asked(), 1);
    assert_eq!(audit.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn review_notice_missing_metadata_is_a_successful_hidden_selection() {
    let audit = Arc::new(RecordingReviewNoticeAudit::default());
    let mut fixture = ReviewFixture::new(audit.clone(), Arc::new(ManualAuditClock::default()));
    let policy = Arc::new(ObservedPolicy::default());
    fixture.state.conversation_access = Some(policy.clone());
    fixture.repository.records.lock().unwrap().clear();
    let hidden = notice_snapshot(
        fixture.board.clone(),
        fixture.state.clone(),
        authenticate(&fixture.state).await,
    )
    .await;
    assert!(hidden.reviews.is_empty());
    assert_eq!(policy.asked(), 0);
    assert!(audit.records.lock().unwrap().is_empty());
}
