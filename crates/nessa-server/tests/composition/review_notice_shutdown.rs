//! Host cleanup observes the application-owned review selection lifetime.
use super::*;
use crate::conversation::application::{
    ReviewNoticeAudit, ReviewNoticeAuditCause, ReviewNoticeAuditFailure, ReviewNoticeAuditRecord,
    SelectReviewNotice,
};
use crate::conversation::infrastructure::{DurableReviewNoticeAudit, MonotonicReviewNoticeClock};
use crate::conversation_test_support::RecordingReviewNoticeAudit;
use crate::core::{Outcome, ShutdownStage};
use crate::product::HostProductFixture;
use nessa_auth::application::{
    ports::{Clock, CredentialEvidence},
    session::AuthenticateSession,
};
use nessa_sdk::domain::agent_execution::{executions::ExecutionId, permissions::PermissionId};
use std::{future::Ready, pin::Pin, sync::Condvar};
use tokio::sync::{Notify, Semaphore};

#[tokio::test]
async fn review_audit_admission_is_fenced_and_pending_metadata_prevents_confirmation() {
    let fixture = HostProductFixture::new().await;
    let auditor = Arc::new(ReviewNoticeAuditor::new(
        Arc::new(RecordingReviewNoticeAudit::default()),
        Arc::new(MonotonicReviewNoticeClock::new()),
    ));
    let admitted = auditor.admit().await.unwrap();
    let state = fixture.state().with_review_notice_auditor(auditor.clone());
    let report = Mutex::new(None);
    cleanup_product(
        &report,
        &state,
        async { Ok(()) },
        async { Ok(()) },
        None::<Ready<Result<(), ConversationError>>>,
        async { Ok(()) },
        async { Ok(()) },
        Duration::from_secs(2),
    )
    .await;
    assert!(
        !read_report(&report, ShutdownReport::confirmed),
        "an admitted selection can still reach Cedar and audit after false confirmed shutdown"
    );
    assert!(
        auditor.admit().await.is_err(),
        "shutdown must fence new policy selection"
    );
    assert_eq!(
        read_report(&report, |r| *r.review_audit()),
        Outcome::Failed(ReviewNoticeAuditFailure::TimedOut)
    );
    drop(admitted);
    assert_eq!(
        auditor.drain().await,
        Err(ReviewNoticeAuditFailure::TimedOut)
    );
}

#[tokio::test(start_paused = true)]
async fn review_audit_ready_completion_wins_expired_deadline() {
    let auditor = ReviewNoticeAuditor::new(
        Arc::new(RecordingReviewNoticeAudit::default()),
        Arc::new(MonotonicReviewNoticeClock::new()),
    );
    let admitted = auditor.admit().await.unwrap();
    auditor.close();
    let slot = Mutex::new(None);
    let (release, released) = tokio::sync::oneshot::channel();
    let mut cleanup = Box::pin(passive_cleanup(
        None,
        &slot,
        Some(&auditor),
        async {
            released.await.unwrap();
            drop(admitted);
            Ok(())
        },
        async { Ok(()) },
        None::<Ready<Result<(), ConversationError>>>,
        async { Ok(()) },
        async { Ok(()) },
        Duration::from_millis(10),
    ));
    assert!(futures_util::poll!(cleanup.as_mut()).is_pending());
    tokio::time::advance(Duration::from_millis(11)).await;
    release.send(()).unwrap();
    cleanup.await;
    assert!(read_report(&slot, ShutdownReport::confirmed));
}

// Block the real worker's physical clock, after its admission but before write.
#[derive(Default)]
struct PhysicalClock {
    released: Mutex<bool>,
    changed: Condvar,
    entered: Notify,
}
impl PhysicalClock {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
impl Clock for PhysicalClock {
    fn unix_seconds(&self) -> u64 {
        self.unix_milliseconds() / 1_000
    }
    fn unix_milliseconds(&self) -> u64 {
        let mut released = self.released.lock().unwrap();
        self.entered.notify_one();
        while !*released {
            released = self.changed.wait(released).unwrap();
        }
        100_000
    }
}
struct ReleasePhysical(Arc<PhysicalClock>);
impl Drop for ReleasePhysical {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct ObservePhysical {
    sink: DurableReviewNoticeAudit,
    entered: Semaphore,
}
impl ReviewNoticeAudit for ObservePhysical {
    fn record(
        &self,
        record: Arc<ReviewNoticeAuditRecord>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send + '_>> {
        Box::pin(async move {
            self.entered.add_permits(1);
            self.sink.record(record).await
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_audit_host_shutdown_joins_physical_write_and_queued_selection() {
    let directory = tempfile::tempdir().unwrap();
    let physical = Arc::new(PhysicalClock::default());
    let _release = ReleasePhysical(physical.clone());
    let sink = Arc::new(ObservePhysical {
        sink: DurableReviewNoticeAudit::new(
            directory.path().join("audit"),
            physical.clone(),
            Uuid::from_u128(10),
        )
        .unwrap(),
        entered: Semaphore::new(0),
    });
    let auditor = Arc::new(ReviewNoticeAuditor::new(
        sink.clone(),
        Arc::new(MonotonicReviewNoticeClock::new()),
    ));
    let fixture = HostProductFixture::new().await;
    let state = fixture.state().with_review_notice_auditor(auditor);
    let session = AuthenticateSession {
        policy: &nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
        verifier: state.verifier.as_ref(),
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
    }
    .execute(
        &CredentialEvidence::new(b"secret".to_vec()).unwrap(),
        state.audience(),
    )
    .await
    .unwrap();
    let mut selections = Vec::new();
    for index in 0..2 {
        let selecting = state.clone();
        let session = session.clone();
        let id = fixture.scope().conversation_id;
        selections.push(tokio::spawn(async move {
            SelectReviewNotice::new(
                selecting.review_metadata.as_ref().unwrap(),
                selecting.access.as_ref(),
                selecting.clock.as_ref(),
                selecting.conversation_access.as_ref().unwrap().as_ref(),
                selecting.review_auditor.as_ref().unwrap(),
            )
            .execute(
                &session,
                &id,
                ExecutionId::new("turn").unwrap(),
                PermissionId::new(format!("permission-{index}")).unwrap(),
                ReviewNoticeAuditCause::SnapshotRefresh,
            )
            .await
        }));
        tokio::time::timeout(Duration::from_secs(2), sink.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        if index == 0 {
            tokio::time::timeout(Duration::from_secs(2), physical.entered.notified())
                .await
                .unwrap();
        }
    }
    let report = Arc::new(Mutex::new(None));
    let output = report.clone();
    let mut cleanup = tokio::spawn(async move {
        cleanup_product(
            &output,
            &state,
            async { Ok(()) },
            async { Ok(()) },
            None::<Ready<Result<(), ConversationError>>>,
            async { Ok(()) },
            async { Ok(()) },
            Duration::from_secs(3),
        )
        .await;
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if report
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|r| r.readers().complete())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut cleanup)
            .await
            .is_err(),
        "host cleanup must remain pending while both policy records are unacknowledged"
    );
    assert!(
        !read_report(&report, ShutdownReport::confirmed),
        "held and queued policy records cannot be confirmed by host cleanup"
    );
    assert!(
        !cleanup.is_finished(),
        "host must join the original audit owner"
    );
    physical.release();
    for selection in selections {
        assert!(selection.await.unwrap().unwrap().is_some());
    }
    tokio::time::timeout(Duration::from_secs(2), cleanup)
        .await
        .unwrap()
        .unwrap();
    assert!(shutdown_result(&report).is_ok());
    assert_eq!(
        std::fs::read_dir(directory.path().join("audit"))
            .unwrap()
            .count(),
        2
    );
}

#[tokio::test]
async fn review_audit_cancelled_cleanup_retains_unknown_outcome() {
    let auditor = ReviewNoticeAuditor::new(
        Arc::new(RecordingReviewNoticeAudit::default()),
        Arc::new(MonotonicReviewNoticeClock::new()),
    );
    let admitted = auditor.admit().await.unwrap();
    auditor.close();
    let slot = Mutex::new(None);
    let mut cleanup = Box::pin(passive_cleanup(
        None,
        &slot,
        Some(&auditor),
        async { Ok(()) },
        async { Ok(()) },
        None::<Ready<Result<(), ConversationError>>>,
        async { Ok(()) },
        async { Ok(()) },
        Duration::from_secs(30),
    ));
    assert!(futures_util::poll!(cleanup.as_mut()).is_pending());
    drop(cleanup);
    drop(admitted);
    assert_eq!(auditor.drain().await, Ok(()));
    assert!(read_report(&slot, |r| r.stage() == ShutdownStage::Drains
        && r.review_audit() == &Outcome::Unknown
        && r.readers().confirmed()));
    assert!(shutdown_result(&slot).is_err());
}

#[tokio::test(start_paused = true)]
async fn review_audit_ready_at_zero_deadline_confirms() {
    let auditor = ReviewNoticeAuditor::new(
        Arc::new(RecordingReviewNoticeAudit::default()),
        Arc::new(MonotonicReviewNoticeClock::new()),
    );
    auditor.close();
    let slot = Mutex::new(None);
    passive_cleanup(
        None,
        &slot,
        Some(&auditor),
        async { Ok(()) },
        async { Ok(()) },
        None::<Ready<Result<(), ConversationError>>>,
        async { Ok(()) },
        async { Ok(()) },
        Duration::ZERO,
    )
    .await;
    assert!(read_report(&slot, ShutdownReport::confirmed));
}

struct RejectAudit;
impl ReviewNoticeAudit for RejectAudit {
    fn record(
        &self,
        _: Arc<ReviewNoticeAuditRecord>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send + '_>> {
        Box::pin(async { Err(ReviewNoticeAuditFailure::Unavailable) })
    }
}

#[tokio::test]
async fn review_audit_host_deadline_preserves_first_delivery_failure() {
    let fixture = HostProductFixture::new().await;
    let auditor = Arc::new(ReviewNoticeAuditor::new(
        Arc::new(RejectAudit),
        Arc::new(MonotonicReviewNoticeClock::new()),
    ));
    let state = fixture.state().with_review_notice_auditor(auditor.clone());
    let session = AuthenticateSession {
        policy: &nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
        verifier: state.verifier.as_ref(),
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
    }
    .execute(
        &CredentialEvidence::new(b"secret".to_vec()).unwrap(),
        state.audience(),
    )
    .await
    .unwrap();
    assert!(matches!(
        SelectReviewNotice::new(
            state.review_metadata.as_ref().unwrap(),
            state.access.as_ref(),
            state.clock.as_ref(),
            state.conversation_access.as_ref().unwrap().as_ref(),
            auditor.as_ref(),
        )
        .execute(
            &session,
            &fixture.scope().conversation_id,
            ExecutionId::new("turn").unwrap(),
            PermissionId::new("rejected").unwrap(),
            ReviewNoticeAuditCause::SnapshotRefresh
        )
        .await,
        Err(ReviewNoticeAuditFailure::Unavailable)
    ));
    let admitted = auditor.admit().await.unwrap();
    let report = Mutex::new(None);
    cleanup_product(
        &report,
        &state,
        async { Ok(()) },
        async { Ok(()) },
        None::<Ready<Result<(), ConversationError>>>,
        async { Ok(()) },
        async { Ok(()) },
        Duration::from_millis(10),
    )
    .await;
    assert_eq!(
        read_report(&report, |r| *r.review_audit()),
        Outcome::Failed(ReviewNoticeAuditFailure::Unavailable)
    );
    drop(admitted);
    assert_eq!(
        auditor.drain().await,
        Err(ReviewNoticeAuditFailure::Unavailable)
    );
}
