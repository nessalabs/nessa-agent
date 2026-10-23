use super::*;
use crate::webhooks::application::{
    ExpiryObserver, MacKey, QueueInitiator, Registration, StopReason, Target, PENDING_LIMIT,
    RETAINED_LIMIT,
};
use crate::webhooks::domain::{Destination, Observation, Outcome};
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{Error, ErrorCode};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::{Id, Scope};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Waker};
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    path: PathBuf,
    audit: PathBuf,
    store: SqlOutbox,
    registration: Registration,
    conversation: ConversationId,
    clock: Arc<Time>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("outbox.db");
        let audit = private.join("audit");
        let clock = Arc::new(Time(AtomicU64::new(10000)));
        let store = SqlOutbox::open(&path, audit.clone(), clock.clone()).unwrap();
        let registration = Registration {
            id: Uuid::new_v4(),
            target: Target {
                credential: CredentialId::new("synthetic-hook-key").unwrap(),
                principal: PrincipalId::new("synthetic-hook").unwrap(),
                organization: OrganizationId::new("synthetic-org").unwrap(),
                owner: PrincipalId::new("synthetic-owner").unwrap(),
            },
            destination: Destination::parse("https://recipient.test/private-synthetic-path")
                .unwrap(),
            key: Arc::new(MacKey::new([19; 32])),
        };
        store.install(registration.clone(), 1000).await.unwrap();
        Self {
            clock,
            _root: root,
            path,
            audit,
            store,
            registration,
            conversation: ConversationId::new(&Uuid::new_v4().to_string()).unwrap(),
        }
    }
    fn record(&self, position: u64) -> QueuedRecord {
        let id = |s: String| Id::new(s).unwrap();
        QueuedRecord {
            registration: self.registration.id,
            conversation: self.conversation.clone(),
            scope: Scope::new(
                id(self.registration.id.to_string()),
                id("origin".into()),
                id(self.conversation.to_string()),
                id("incarnation-a".into()),
                id("records-v1".into()),
                id("epoch-7".into()),
            ),
            expected_after: position - 1,
            position,
            epoch: 7,
            body: format!("{{\"syntheticPosition\":{position}}}").into_bytes(),
            id: Uuid::new_v4(),
            observed_at_ms: 1000 + position,
            initiator: QueueInitiator::Live,
        }
    }
    fn audits(&self) -> Vec<Value> {
        std::fs::read_dir(&self.audit)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let text = std::fs::read_to_string(path).unwrap();
                assert!(!text.contains("private-synthetic-path"));
                assert!(!text.contains("syntheticPosition"));
                assert!(!text.contains("mac_key"));
                serde_json::from_str(&text).unwrap()
            })
            .collect()
    }
}

#[tokio::test]
async fn checkpoint_and_identity_survive_reopen() {
    let mut f = Fixture::new().await;
    let id = f.store.queue(f.record(1)).await.unwrap();
    assert_eq!(f.store.queue(f.record(1)).await.unwrap(), id);
    assert_eq!(f.audits().len(), 2);
    let reopened = {
        let placeholder = Fixture::new().await;
        let previous = std::mem::replace(&mut f.store, placeholder.store);
        drop(previous);
        SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap()
    };
    let work = reopened.due(2000).await.unwrap();
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].delivery.id(), id);
    assert_eq!(work[0].body, f.record(1).body);
    let second = reopened.queue(f.record(2)).await.unwrap();
    assert_ne!(id, second);
    assert_eq!(reopened.due(2000).await.unwrap().len(), 2);
}

#[tokio::test]
async fn conflicting_checkpoint_does_not_replace_bytes() {
    let f = Fixture::new().await;
    let id = f.store.queue(f.record(1)).await.unwrap();
    let mut changed = f.record(1);
    changed.body = b"different".to_vec();
    assert_eq!(f.store.queue(changed).await, Err(OutboxError::Conflict));
    let mut wrong_epoch = f.record(1);
    wrong_epoch.epoch = 8;
    assert_eq!(f.store.queue(wrong_epoch).await, Err(OutboxError::Invalid));
    assert_eq!(f.store.queue(f.record(1)).await.unwrap(), id);
    assert_eq!(f.audits().len(), 2);
}

#[tokio::test]
async fn cursor_cas_and_queue_limit_preserve_predecessor() {
    let f = Fixture::new().await;
    assert_eq!(f.store.queue(f.record(2)).await, Err(OutboxError::Conflict));
    for position in 1..=PENDING_LIMIT as u64 {
        f.store.queue(f.record(position)).await.unwrap();
    }
    assert_eq!(
        f.store.queue(f.record(PENDING_LIMIT as u64 + 1)).await,
        Err(OutboxError::Capacity)
    );
    assert_eq!(f.store.due(2000).await.unwrap().len(), 4);
    let connection = f.store.owner.connection.lock().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT position FROM cursors", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        PENDING_LIMIT as i64
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM deliveries", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        PENDING_LIMIT as i64
    );
}

#[tokio::test]
async fn source_change_requires_explicit_reset() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    f.store.queue(f.record(2)).await.unwrap();
    let work = f.store.due(2000).await.unwrap();
    let sending = work[0].delivery.admit(7, 2000).unwrap();
    f.store
        .transition(&work[0].delivery, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    let mut reset = f.record(3);
    reset.scope = Scope::new(
        reset.scope.receiver().clone(),
        reset.scope.origin().clone(),
        reset.scope.stream().clone(),
        Id::new("incarnation-b").unwrap(),
        reset.scope.schema().clone(),
        reset.scope.access_epoch().clone(),
    );
    assert_eq!(f.store.queue(reset).await, Err(OutboxError::ResetRequired));
    assert_eq!(
        f.store.queue(f.record(3)).await,
        Err(OutboxError::ResetRequired)
    );
    assert!(f.store.due(3000).await.unwrap().is_empty());
    let sent = sending.observed(Observation::Success, 2500).unwrap();
    f.store
        .transition(&sending, &sent, DeliveryCause::Response, 2500)
        .await
        .unwrap();
    let audits = f.audits();
    assert_eq!(
        audits
            .iter()
            .filter(|a| a["kind"] == "webhook_cursor" && a["after"] == "reset_required")
            .count(),
        1
    );
    assert_eq!(
        audits
            .iter()
            .filter(|a| a["after"] == "not_sent" && a["cause"] == "source_changed")
            .count(),
        1
    );
}

#[tokio::test]
async fn transition_cas_rejects_lost_ack_repost() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .transition(&queued, &sending, DeliveryCause::Admission, 2000)
            .await,
        Err(OutboxError::Conflict)
    );
    assert!(f.store.due(u64::MAX / 2).await.unwrap().is_empty());
    let retry = sending.observed(Observation::Retryable, 2001).unwrap();
    f.store
        .transition(&sending, &retry, DeliveryCause::Response, 2001)
        .await
        .unwrap();
    assert!(f.store.due(3000).await.unwrap().is_empty());
    let due = f.store.due(3001).await.unwrap();
    assert_eq!(due[0].delivery.id(), queued.id());
    assert_eq!(due[0].delivery.timestamp_ms(), Some(2000));
    assert_eq!(due[0].delivery.attempts(), 1);
}

#[tokio::test]
async fn restart_recovers_sending_without_repost() {
    let mut f = Fixture::new().await;
    let id = f.store.queue(f.record(1)).await.unwrap();
    f.store.queue(f.record(2)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    let replacement = Fixture::new().await;
    drop(std::mem::replace(&mut f.store, replacement.store));
    f.store = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
    f.store.recover(3000).await.unwrap();
    f.store.recover(3001).await.unwrap();
    let due = f.store.due(4000).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_ne!(due[0].delivery.id(), id);
    let audit = f.audits();
    assert_eq!(
        audit
            .iter()
            .filter(|a| a["deliveryId"] == id.to_string()
                && a["after"] == "unknown"
                && a["cause"] == "restart"
                && a["initiator"] == "restart")
            .count(),
        1
    );
}

#[tokio::test]
async fn stop_preserves_inflight_observation_and_cause() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    f.store.queue(f.record(2)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    let credential = f.registration.target.credential.clone();
    f.store
        .stop(
            credential.clone(),
            StopReason::Revoked {
                initiator: PrincipalId::new("canonical-revoker").unwrap(),
            },
            2001,
        )
        .await
        .unwrap();
    f.store
        .stop(
            credential.clone(),
            StopReason::Expired {
                observed_by: ExpiryObserver::Delivery(Uuid::new_v4()),
            },
            2002,
        )
        .await
        .unwrap();
    assert!(f.store.registration(credential).await.unwrap().is_none());
    assert_eq!(f.store.queue(f.record(3)).await, Err(OutboxError::Conflict));
    assert!(f.store.due(5000).await.unwrap().is_empty());
    let sent = sending.observed(Observation::Success, 2100).unwrap();
    f.store
        .transition(&sending, &sent, DeliveryCause::Response, 2100)
        .await
        .unwrap();
    let audits = f.audits();
    assert_eq!(
        audits
            .iter()
            .filter(|a| a["after"] == "not_sent"
                && a["cause"] == "credential_revoked"
                && a["initiator"] == "canonical-revoker")
            .count(),
        1
    );
    assert!(!audits.iter().any(|a| a["cause"] == "credential_expired"));
    let retired = audits
        .iter()
        .find(|a| a["before"] == "stopped" && a["after"] == "absent")
        .unwrap();
    assert_eq!(retired["initiator"], "canonical-revoker");
    assert_eq!(retired["stopAtMs"], 2001);
    assert_eq!(retired["cause"], "drained");
    assert_eq!(retired["stopCause"], "credential_revoked");
    let connection = f.store.owner.connection.lock().unwrap();
    for table in ["registrations", "cursors", "deliveries"] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn audit_failure_recovers_before_work() {
    let f = Fixture::new().await;
    // An unavailable projection after SQL commit must not acknowledge queue.
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store.queue(f.record(1)).await,
        Err(OutboxError::Unavailable)
    );
    assert!(matches!(
        f.store.due(2000).await,
        Err(OutboxError::Unavailable)
    ));
    std::fs::rename(&held, &f.audit).unwrap();
    let due = f.store.due(2000).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(
        f.store.queue(f.record(1)).await.unwrap(),
        due[0].delivery.id()
    );
    assert_eq!(f.audits().len(), 2);
}

#[tokio::test]
async fn owner_lock_and_physical_permit_survive_waiter_cancellation() {
    let f = Fixture::new().await;
    assert!(matches!(
        SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
        Err(OutboxError::Unavailable)
    ));
    {
        let connection = f.store.owner.connection.lock().unwrap();
        let mut pending = Box::pin(f.store.due(1000));
        assert!(matches!(
            pending
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        drop(pending);
        assert_eq!(f.store.owner.capacity.available_permits(), 3);
        drop(connection);
    }
    // Reconcile the retained physical job before final accounting.
    for _ in 0..100 {
        if f.store.owner.capacity.available_permits() == 4 {
            return;
        }
        tokio::task::yield_now().await;
    }
    let _permit = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        f.store.owner.capacity.acquire_many(4),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn terminal_retention_is_bounded() {
    let f = Fixture::new().await;
    for position in 1..=RETAINED_LIMIT as u64 + 1 {
        f.store.queue(f.record(position)).await.unwrap();
        let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
        let sending = queued.admit(7, 2000).unwrap();
        f.store
            .transition(&queued, &sending, DeliveryCause::Admission, 2000)
            .await
            .unwrap();
        let sent = sending.observed(Observation::Success, 2001).unwrap();
        f.store
            .transition(&sending, &sent, DeliveryCause::Response, 2001)
            .await
            .unwrap();
        assert_eq!(sent.phase(), Phase::Terminal(Outcome::Sent));
    }
    let connection = f.store.owner.connection.lock().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM deliveries", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        RETAINED_LIMIT as i64
    );
    assert!(
        connection
            .query_row("SELECT count(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap()
            <= 3 * RETAINED_LIMIT as i64
    );
    drop(connection);
    assert_eq!(f.audits().len(), 1 + 3 * (RETAINED_LIMIT + 1));
}

#[tokio::test]
async fn invalid_replacements_and_restore_combinations_are_refused() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    let sent = sending.observed(Observation::Success, 2001).unwrap();
    assert_eq!(
        f.store
            .transition(&queued, &sent, DeliveryCause::Response, 2001)
            .await,
        Err(OutboxError::Invalid)
    );
    assert_eq!(
        f.store
            .transition(&queued, &sending, DeliveryCause::Restart, 2000)
            .await,
        Err(OutboxError::Invalid)
    );
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .transition(&sending, &queued, DeliveryCause::Admission, 2001)
            .await,
        Err(OutboxError::Invalid)
    );
    {
        let connection = f.store.owner.connection.lock().unwrap();
        let mut unreachable: Value =
            serde_json::from_str(&codec::encode(&queued).unwrap()).unwrap();
        unreachable["phase"] = json!("sending");
        connection
            .execute("UPDATE deliveries SET state=?1", [unreachable.to_string()])
            .unwrap();
    }
    assert_eq!(f.store.recover(3000).await, Err(OutboxError::Invalid));
    {
        let connection = f.store.owner.connection.lock().unwrap();
        assert!(connection
            .execute("UPDATE deliveries SET phase='queued',due_at=99", [])
            .is_err());
    }
}

#[tokio::test]
async fn failed_audit_insert_rolls_back_checkpoint_and_delivery() {
    let f = Fixture::new().await;
    {
        let connection = f.store.owner.connection.lock().unwrap();
        connection.execute_batch("CREATE TRIGGER reject_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'synthetic audit refusal'); END;").unwrap();
    }
    assert_eq!(
        f.store.queue(f.record(1)).await,
        Err(OutboxError::Unavailable)
    );
    assert!(f.store.due(2000).await.unwrap().is_empty());
    {
        let connection = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM cursors", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        connection
            .execute_batch("DROP TRIGGER reject_audit;")
            .unwrap();
    }
    f.store.queue(f.record(1)).await.unwrap();
    assert_eq!(f.store.due(2000).await.unwrap().len(), 1);
}

fn replaced_source(mut record: QueuedRecord) -> QueuedRecord {
    record.scope = Scope::new(
        record.scope.receiver().clone(),
        record.scope.origin().clone(),
        record.scope.stream().clone(),
        Id::new("incarnation-b").unwrap(),
        record.scope.schema().clone(),
        record.scope.access_epoch().clone(),
    );
    record
}

#[tokio::test]
async fn late_retry_after_stop_does_not_become_ready() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    f.store
        .stop(
            f.registration.target.credential.clone(),
            StopReason::Revoked {
                initiator: PrincipalId::new("original-owner").unwrap(),
            },
            2001,
        )
        .await
        .unwrap();
    f.clock.0.store(2100, Ordering::SeqCst);
    let retry = sending.observed(Observation::Retryable, 2100).unwrap();
    f.store
        .transition(&sending, &retry, DeliveryCause::Response, 2100)
        .await
        .unwrap();
    assert!(f.store.due(4000).await.unwrap().is_empty());
    assert!(f
        .store
        .registration(f.registration.target.credential.clone())
        .await
        .unwrap()
        .is_none());
    let audits = f.audits();
    assert_eq!(
        audits
            .iter()
            .filter(|a| a["before"] == "sending"
                && a["after"] == "retry"
                && a["cause"] == "response")
            .count(),
        1
    );
    let refusal = audits
        .iter()
        .find(|a| a["before"] == "retry" && a["after"] == "not_sent")
        .unwrap();
    assert_eq!(refusal["cause"], "credential_revoked");
    assert_eq!(refusal["initiator"], "original-owner");
    assert_eq!(refusal["atMs"], 2100);
}

#[tokio::test]
async fn bulk_refusal_rejects_foreign_state_without_partial_effects() {
    let f = Fixture::new().await;
    let first = f.store.queue(f.record(1)).await.unwrap();
    let second = f.store.queue(f.record(2)).await.unwrap();
    {
        let connection = f.store.owner.connection.lock().unwrap();
        connection.execute("UPDATE deliveries SET state=(SELECT state FROM deliveries WHERE id=?1) WHERE id=?2",params![second.to_string(),first.to_string()]).unwrap();
    }
    assert_eq!(
        f.store
            .stop(
                f.registration.target.credential.clone(),
                StopReason::Revoked {
                    initiator: PrincipalId::new("owner").unwrap()
                },
                2000
            )
            .await,
        Err(OutboxError::Invalid)
    );
    let connection = f.store.owner.connection.lock().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT active FROM registrations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM deliveries WHERE phase='queued'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    drop(connection);
    assert_eq!(f.audits().len(), 3);
}

#[tokio::test]
async fn cursor_scope_identity_is_not_overwritten() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let original = f.record(1).scope;
    let foreign = Scope::new(
        Id::new(Uuid::new_v4().to_string()).unwrap(),
        original.origin().clone(),
        original.stream().clone(),
        original.incarnation().clone(),
        original.schema().clone(),
        original.access_epoch().clone(),
    );
    {
        let connection = f.store.owner.connection.lock().unwrap();
        connection
            .execute(
                "UPDATE cursors SET scope=?1",
                [codec::scope_text(&foreign).unwrap()],
            )
            .unwrap();
    }
    assert_eq!(f.store.queue(f.record(2)).await, Err(OutboxError::Invalid));
    let connection = f.store.owner.connection.lock().unwrap();
    let cursor: String = connection
        .query_row("SELECT scope FROM cursors", [], |r| r.get(0))
        .unwrap();
    assert_eq!(codec::scope(&cursor).unwrap(), foreign);
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM deliveries", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn restored_rows_are_checked_before_projection_cleanup() {
    let mut f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    {
        let connection = f.store.owner.connection.lock().unwrap();
        assert!(connection
            .execute("UPDATE deliveries SET phase='sent'", [])
            .is_err());
        connection
            .execute("UPDATE deliveries SET incarnation='wrong-incarnation'", [])
            .unwrap();
    }
    assert!(matches!(f.store.due(2000).await, Err(OutboxError::Invalid)));
    {
        let connection = f.store.owner.connection.lock().unwrap();
        // A contradictory terminal state would be hidden by a ready-only scan.
        connection
            .execute(
                "UPDATE deliveries SET state=json_set(state,'$.phase','sent')",
                [],
            )
            .unwrap();
    }
    let replacement = Fixture::new().await;
    drop(std::mem::replace(&mut f.store, replacement.store));
    assert!(matches!(
        SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
        Err(OutboxError::Invalid)
    ));
    assert_eq!(f.audits().len(), 2);
}

#[tokio::test]
async fn bulk_reset_keeps_one_reader_initiator() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    f.store.queue(f.record(2)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    let retry = sending.observed(Observation::Retryable, 2001).unwrap();
    f.store
        .transition(&sending, &retry, DeliveryCause::Response, 2001)
        .await
        .unwrap();
    let mut reset = replaced_source(f.record(3));
    reset.initiator = QueueInitiator::Restart;
    assert_eq!(f.store.queue(reset).await, Err(OutboxError::ResetRequired));
    let audits = f.audits();
    let changed: Vec<_> = audits
        .iter()
        .filter(|a| a["cause"] == "source_changed")
        .collect();
    assert_eq!(changed.len(), 3);
    assert!(changed.iter().all(|a| a["initiator"] == "restart"));
    assert!(changed
        .iter()
        .any(|a| a["before"] == "retry" && a["after"] == "not_sent"));
    assert!(changed
        .iter()
        .any(|a| a["before"] == "queued" && a["after"] == "not_sent"));
}

#[tokio::test]
async fn bulk_reset_retention_is_bounded_in_the_commit() {
    let f = Fixture::new().await;
    for position in 1..=RETAINED_LIMIT as u64 {
        f.store.queue(f.record(position)).await.unwrap();
        let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
        let refused = queued.refused().unwrap();
        f.store
            .transition(&queued, &refused, DeliveryCause::Refused, 1001)
            .await
            .unwrap();
    }
    for position in RETAINED_LIMIT as u64 + 1..=(RETAINED_LIMIT + PENDING_LIMIT) as u64 {
        f.store.queue(f.record(position)).await.unwrap();
    }
    let next = (RETAINED_LIMIT + PENDING_LIMIT) as u64 + 1;
    assert_eq!(
        f.store.queue(replaced_source(f.record(next))).await,
        Err(OutboxError::ResetRequired)
    );
    // Read the committed DB directly, without a public operation's recovery
    // pass masking a transient retention violation.
    let connection = f.store.owner.connection.lock().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM deliveries WHERE phase='not_sent'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        RETAINED_LIMIT as i64
    );
    assert_eq!(
        connection
            .query_row("SELECT position FROM cursors", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        (RETAINED_LIMIT + PENDING_LIMIT) as i64
    );
}

#[test]
fn restored_retry_respects_backoff_without_assuming_wall_clock_order() {
    let id = Uuid::new_v4();
    assert!(Delivery::restore(id, 7, Phase::Retry { due_at_ms: 999 }, 1, Some(20000)).is_err());
    // A backward wall-clock step can put due before the original timestamp.
    assert!(Delivery::restore(id, 7, Phase::Retry { due_at_ms: 1000 }, 1, Some(20000)).is_ok());
}

#[tokio::test]
async fn a_file_without_generated_projections_is_not_the_current_schema() {
    let f = Fixture::new().await;
    let path = f.path.with_file_name("wrong-shape.db");
    let ordinary=SCHEMA.replace("phase TEXT GENERATED ALWAYS AS (json_extract(state, '$.phase')) STORED NOT NULL", "phase TEXT NOT NULL")
        .replace("due_at INTEGER GENERATED ALWAYS AS (json_extract(state, '$.due_at')) STORED NOT NULL CHECK(due_at >= 0)", "due_at INTEGER NOT NULL");
    let file = nessa_local_storage::open(&path, OpenMode::OpenOrCreate).unwrap();
    drop(file);
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(&ordinary).unwrap();
    drop(connection);
    assert!(matches!(
        SqlOutbox::open(
            &path,
            f.audit.with_file_name("wrong-shape-audit"),
            f.clock.clone()
        ),
        Err(OutboxError::Invalid)
    ));
}

#[tokio::test]
async fn late_retry_keeps_source_refusal_after_cursor_replacement() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    f.store.queue(f.record(2)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    let mut changed = replaced_source(f.record(3));
    changed.initiator = QueueInitiator::Restart;
    assert_eq!(
        f.store.queue(changed).await,
        Err(OutboxError::ResetRequired)
    );
    {
        // Isolated fixture models a later authorized cursor reset; there is no
        // production reset API or authorization claim in this store test.
        let connection = f.store.owner.connection.lock().unwrap();
        connection.execute("DELETE FROM cursors", []).unwrap();
    }
    f.clock.0.store(2200, Ordering::SeqCst);
    let retry = sending.observed(Observation::Retryable, 2200).unwrap();
    f.store
        .transition(&sending, &retry, DeliveryCause::Response, 2200)
        .await
        .unwrap();
    assert!(f.store.due(4000).await.unwrap().is_empty());
    let audits = f.audits();
    assert_eq!(
        audits
            .iter()
            .filter(|a| a["before"] == "sending"
                && a["after"] == "retry"
                && a["cause"] == "response")
            .count(),
        1
    );
    let refused = audits
        .iter()
        .find(|a| a["before"] == "retry" && a["after"] == "not_sent")
        .unwrap();
    assert_eq!(refused["cause"], "source_changed");
    assert_eq!(refused["initiator"], "restart");
    assert_eq!(refused["sourceStop"]["at_ms"], 1003);
    let mut malformed = refused.clone();
    malformed["sourceStop"]["initiator"] = json!("live");
    assert!(matches!(
        audit::Record::decode(&malformed.to_string()),
        Err(OutboxError::Invalid)
    ));
    assert_eq!(refused["atMs"], 2200);
    assert!(!audits.iter().any(|a| a["before"] == "sending"
        && a["after"] == "not_sent"
        && a["cause"] == "source_changed"));
}

#[tokio::test]
async fn restart_keeps_invalidated_sending_unknown() {
    let mut f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    assert_eq!(
        f.store.queue(replaced_source(f.record(2))).await,
        Err(OutboxError::ResetRequired)
    );
    let replacement = Fixture::new().await;
    drop(std::mem::replace(&mut f.store, replacement.store));
    f.store = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
    f.store.recover(3000).await.unwrap();
    assert!(f.store.due(4000).await.unwrap().is_empty());
    let audits = f.audits();
    assert_eq!(
        audits
            .iter()
            .filter(|a| a["after"] == "unknown" && a["cause"] == "restart")
            .count(),
        1
    );
    assert!(!audits.iter().any(|a| a["after"] == "not_sent"));
}

#[tokio::test]
async fn stop_retention_is_bounded_when_publication_fails() {
    let f = Fixture::new().await;
    for position in 1..=RETAINED_LIMIT as u64 {
        f.store.queue(f.record(position)).await.unwrap();
        let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
        f.store
            .transition(
                &queued,
                &queued.refused().unwrap(),
                DeliveryCause::Refused,
                1001,
            )
            .await
            .unwrap();
    }
    for position in RETAINED_LIMIT as u64 + 1..=(RETAINED_LIMIT + PENDING_LIMIT) as u64 {
        f.store.queue(f.record(position)).await.unwrap();
    }
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store
            .stop(
                f.registration.target.credential.clone(),
                StopReason::Revoked {
                    initiator: PrincipalId::new("canonical-revoker").unwrap()
                },
                2001
            )
            .await,
        Err(OutboxError::Unavailable)
    );
    {
        let connection = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM deliveries", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            PENDING_LIMIT as i64
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM deliveries WHERE position<=?1",
                    [RETAINED_LIMIT as i64],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT active FROM registrations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    std::fs::rename(&held, &f.audit).unwrap();
    assert!(f.store.due(4000).await.unwrap().is_empty());
    assert!(f
        .store
        .registration(f.registration.target.credential.clone())
        .await
        .unwrap()
        .is_none());
    let audits = f.audits();
    let retired = audits
        .iter()
        .find(|a| a["before"] == "stopped" && a["after"] == "absent")
        .unwrap();
    assert_eq!(retired["initiator"], "canonical-revoker");
}

#[tokio::test]
async fn noncanonical_audit_key_is_refused_before_publication() {
    let mut f = Fixture::new().await;
    {
        let connection = f.store.owner.connection.lock().unwrap();
        let id = "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA";
        connection
            .execute(
                "UPDATE audit SET id=?1,record=json_set(record,'$.recordId',?1),published=0",
                [id],
            )
            .unwrap();
    }
    let prior = f.audits().len();
    let replacement = Fixture::new().await;
    drop(std::mem::replace(&mut f.store, replacement.store));
    assert!(matches!(
        SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
        Err(OutboxError::Invalid)
    ));
    assert_eq!(f.audits().len(), prior);
}

#[tokio::test]
async fn aliased_saved_state_is_refused_on_restore() {
    for pretty in [false, true] {
        let mut f = Fixture::new().await;
        let mut record = f.record(1);
        record.id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
        f.store.queue(record).await.unwrap();
        {
            let connection = f.store.owner.connection.lock().unwrap();
            let original: String = connection
                .query_row("SELECT state FROM deliveries", [], |r| r.get(0))
                .unwrap();
            let aliased = if pretty {
                serde_json::to_string_pretty(&serde_json::from_str::<Value>(&original).unwrap())
                    .unwrap()
            } else {
                original.replace(
                    "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                    "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
                )
            };
            connection
                .execute("UPDATE deliveries SET state=?1", [aliased])
                .unwrap();
        }
        let replacement = Fixture::new().await;
        drop(std::mem::replace(&mut f.store, replacement.store));
        assert!(matches!(
            SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
            Err(OutboxError::Invalid)
        ));
    }
}

#[tokio::test]
async fn restored_capacity_is_bounded_before_collection() {
    for terminal in [false, true] {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        {
            let mut connection = f.store.owner.connection.lock().unwrap();
            let transaction = connection.transaction().unwrap();
            for position in 2..=PENDING_LIMIT as u64 + 2 {
                let record = f.record(position);
                let delivery = Delivery::queued(record.id, 7);
                let delivery = if terminal {
                    delivery.refused().unwrap()
                } else {
                    delivery
                };
                transaction.execute("INSERT INTO deliveries(id,registration,conversation,incarnation,position,scope,body,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![record.id.to_string(),record.registration.to_string(),record.conversation.to_string(),record.scope.incarnation().as_str(),position as i64,codec::scope_text(&record.scope).unwrap(),record.body,codec::encode(&delivery).unwrap()]).unwrap();
            }
            transaction
                .execute(
                    "UPDATE cursors SET position=?1",
                    [(PENDING_LIMIT + 2) as i64],
                )
                .unwrap();
            transaction.commit().unwrap();
        }
        let replacement = Fixture::new().await;
        drop(std::mem::replace(&mut f.store, replacement.store));
        assert!(matches!(
            SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
            Err(OutboxError::Invalid)
        ));
    }
    let mut f = Fixture::new().await;
    {
        let mut connection = f.store.owner.connection.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        for n in 1..=crate::webhooks::application::MAX_REGISTRATIONS {
            let id = Uuid::new_v4().to_string();
            let credential = format!("synthetic-key-{n}");
            let principal = format!("synthetic-principal-{n}");
            let request = format!("capacity-fixture-{n}");
            // Preserve the current valid relationship while exceeding the
            // count bound; this is not a dangling-reference demonstration.
            transaction.execute("INSERT INTO registration_intents(id,issuer,request,issue,destination,mac_key,observed_at) SELECT ?1,issuer,?5,json_set(issue,'$.credentialId',?2,'$.principal.id',?3,'$.membership.principalId',?3,'$.requestId',?5),destination,mac_key,observed_at FROM registration_intents WHERE id=?4",params![id,credential,principal,f.registration.id.to_string(),request]).unwrap();
            transaction.execute("INSERT INTO registrations(id,credential,principal,organization,owner,destination,mac_key,active,issued_at) SELECT ?1,?2,?3,organization,owner,destination,mac_key,1,issued_at FROM registrations WHERE id=?4",params![id,credential,principal,f.registration.id.to_string()]).unwrap();
        }
        transaction.commit().unwrap();
    }
    let replacement = Fixture::new().await;
    drop(std::mem::replace(&mut f.store, replacement.store));
    assert!(matches!(
        SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
        Err(OutboxError::Invalid)
    ));
}

#[tokio::test]
async fn cancelled_observer_keeps_physical_worker_and_canonical_owner() {
    let mut f = Fixture::new().await;
    let capacity = f.store.owner.capacity.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let clone = f.store.clone();
    let observer = tokio::spawn(async move {
        clone
            .run(move |_, _| {
                entered.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(3))
                    .map_err(unavailable)?;
                Ok(())
            })
            .await
    });
    ready.await.unwrap();
    observer.abort();
    assert!(observer.await.unwrap_err().is_cancelled());
    let replacement = Fixture::new().await;
    drop(std::mem::replace(&mut f.store, replacement.store));
    assert_eq!(
        capacity.available_permits(),
        crate::webhooks::application::MAX_OUTBOX_WORKERS - 1
    );
    assert!(matches!(
        SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
        Err(OutboxError::Unavailable)
    ));
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()) {
                Ok(store) => break store,
                Err(OutboxError::Unavailable) => tokio::task::yield_now().await,
                Err(error) => panic!("unexpected owner reopen: {error:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        capacity.available_permits(),
        crate::webhooks::application::MAX_OUTBOX_WORKERS
    );
}

#[tokio::test]
async fn audit_acknowledgement_is_exact_and_recoverable() {
    let f = Fixture::new().await;
    {
        let connection = f.store.owner.connection.lock().unwrap();
        connection.execute_batch("CREATE TRIGGER ignore_ack BEFORE UPDATE OF published ON audit BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    }
    // The file can be durable while acknowledgement fails. Refuse promptly;
    // replay verifies those exact bytes and never creates a new delivery id.
    assert_eq!(f.store.queue(f.record(1)).await, Err(OutboxError::Conflict));
    {
        let connection = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM audit WHERE published=0", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        connection
            .execute_batch("DROP TRIGGER ignore_ack;")
            .unwrap();
    }
    let ready = f.store.due(2000).await.unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(
        f.store.queue(f.record(1)).await.unwrap(),
        ready[0].delivery.id()
    );
    assert_eq!(f.audits().len(), 2);
}

#[tokio::test]
async fn audit_record_correlations_are_checked_before_cleanup() {
    for field in ["recordId", "registration", "deliveryId", "kind"] {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        {
            let connection = f.store.owner.connection.lock().unwrap();
            let value = if field == "kind" {
                "webhook_cursor".to_owned()
            } else {
                Uuid::new_v4().to_string()
            };
            connection
                .execute(
                    "UPDATE audit SET record=json_set(record,?1,?2)",
                    params![format!("$.{field}"), value],
                )
                .unwrap();
        }
        let replacement = Fixture::new().await;
        drop(std::mem::replace(&mut f.store, replacement.store));
        assert!(matches!(
            SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
            Err(OutboxError::Invalid)
        ));
        assert_eq!(f.audits().len(), 2);
    }
}

#[tokio::test]
async fn restored_registration_facts_are_checked_before_effects() {
    let cases = [
        "credential=''",
        "principal=''",
        "organization=''",
        "owner=''",
        "destination='http://recipient.test/'",
        "id=upper(id)",
        "active=0,stop_cause='credential_revoked',stop_initiator='',stop_at=1000",
        "active=0,stop_cause='credential_expired',stop_initiator='delivery:invalid',stop_at=1000",
        "active=0,stop_cause='credential_expired',stop_initiator='delivery:AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA',stop_at=1000",
        "active=0,stop_cause='credential_revoked',stop_initiator='synthetic-owner',stop_at='invalid'",
    ];
    for set in cases {
        let mut f = Fixture::new().await;
        let audit_before = f.audits();
        {
            let c = f.store.owner.connection.lock().unwrap();
            let mutation = format!("UPDATE registrations SET {set}");
            if set == "id=upper(id)" {
                assert!(
                    matches!(c.execute(&mutation,[]),Err(Error::SqliteFailure(error,_)) if error.code==ErrorCode::ConstraintViolation)
                );
                // The live foreign key now prevents this mutation. Model an
                // offline damaged file separately to preserve the cold guard.
                c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
            }
            c.execute(&mutation, []).unwrap();
            c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        }
        let placeholder = Fixture::new().await;
        drop(std::mem::replace(&mut f.store, placeholder.store));
        assert!(
            matches!(
                SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
                Err(OutboxError::Invalid)
            ),
            "accepted {set}"
        );
        assert_eq!(f.audits(), audit_before, "published before refusing {set}");
    }
    for (cause, initiator) in [
        ("credential_revoked", "synthetic-owner"),
        (
            "credential_expired",
            "delivery:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        ),
    ] {
        let mut f = Fixture::new().await;
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE registrations SET active=0,stop_cause=?1,stop_initiator=?2,stop_at=?3",
                nessa_local_database::rusqlite::params![cause, initiator, i64::MAX],
            )
            .unwrap();
        let placeholder = Fixture::new().await;
        drop(std::mem::replace(&mut f.store, placeholder.store));
        let reopened = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
        let connection = reopened.owner.connection.lock().unwrap();
        let saved = registry::by_id(&connection, f.registration.id).unwrap();
        let stopped = saved.lifecycle.stopped().unwrap();
        assert!(matches!(
            (cause, stopped.reason()),
            ("credential_revoked", StopReason::Revoked { .. })
                | ("credential_expired", StopReason::Expired { .. })
        ));
        assert_eq!(registry::stop_actor(stopped), initiator);
        assert_eq!(stopped.at_ms(), i64::MAX as u64);
    }
}

#[tokio::test]
async fn delivery_audit_uses_checked_registration_facts() {
    let f = Fixture::new().await;
    f.store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute("UPDATE registrations SET principal=''", [])
        .unwrap();
    let audit_before = f.audits();
    assert_eq!(f.store.queue(f.record(1)).await, Err(OutboxError::Invalid));
    assert_eq!(f.audits(), audit_before);
    let connection = f.store.owner.connection.lock().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM deliveries", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM cursors", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

async fn reopen_store(f: &mut Fixture) -> Result<SqlOutbox, OutboxError> {
    let placeholder = Fixture::new().await;
    drop(std::mem::replace(&mut f.store, placeholder.store));
    SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone())
}

#[tokio::test]
async fn malformed_owed_audit_is_refused_before_effects() {
    let cases = [
        ("principal", json!("")),
        ("credential", json!("")),
        ("organization", json!("")),
        ("owner", json!("")),
        ("before", json!("unknown_phase")),
        ("after", json!("unknown_phase")),
        ("atMs", json!(-1)),
        ("atMs", json!("invalid")),
        ("cause", json!("unrecorded_cause")),
        ("initiator", json!("")),
        (
            "initiator",
            json!("reader:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ),
        ("remoteText", json!("synthetic-sensitive-text")),
        ("epoch", json!(8)),
        ("attempt", json!(1)),
        ("timestampMs", json!(1000)),
        ("dueAtMs", json!(1)),
        ("position", json!(0)),
        ("position", json!(u64::MAX)),
        ("conversationId", json!("")),
        ("incarnation", json!("other-source")),
        ("sourceStop", json!({"initiator":"live","at_ms":1000})),
        (
            "credentialStop",
            json!({"cause":"credential_revoked","initiator":"original-revoker","atMs":1000}),
        ),
        ("beforeState", json!({"id":"invalid"})),
        ("afterState", json!({"id":"invalid"})),
        ("scope", json!({"receiver":"invalid"})),
        ("afterState.extra", json!("synthetic-secret")),
        ("afterState.epoch", json!(8)),
        (
            "afterState.id",
            json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ),
        ("afterState.due_at", json!(1)),
        (
            "scope.receiver",
            json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ),
        (
            "scope.stream",
            json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ),
        ("scope.incarnation", json!("different-source")),
        ("scope.access_epoch", json!("epoch-8")),
    ];
    for (field, malformed) in cases {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        let (id, text): (String, String) = f
            .store
            .owner
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT id,record FROM audit WHERE delivery IS NOT NULL LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut record: Value = serde_json::from_str(&text).unwrap();
        if let Some((parent, key)) = field.split_once('.') {
            record[parent][key] = malformed;
        } else {
            record[field] = malformed;
        }
        assert!(
            matches!(
                audit::Record::decode(&record.to_string()),
                Err(OutboxError::Invalid)
            ),
            "typed record accepted {field}"
        );
        if let Ok(typed) = serde_json::from_value::<audit::Record>(record.clone()) {
            assert!(
                matches!(typed.encode(), Err(OutboxError::Invalid)),
                "writer accepted {field}"
            );
        }
        std::fs::remove_file(f.audit.join(format!("{id}.json"))).unwrap();
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE audit SET record=?1,published=0 WHERE id=?2",
                params![record.to_string(), id],
            )
            .unwrap();
        assert!(
            matches!(reopen_store(&mut f).await, Err(OutboxError::Invalid)),
            "accepted {field}"
        );
        assert!(
            !f.audit.join(format!("{id}.json")).exists(),
            "published {field}"
        );
    }
}

#[tokio::test]
async fn impossible_typed_delivery_history_is_refused() {
    let f = Fixture::new().await;
    let id = f.store.queue(f.record(1)).await.unwrap();
    let text: String = f
        .store
        .owner
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT record FROM audit WHERE delivery=?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    let mut record: Value = serde_json::from_str(&text).unwrap();
    let queued = Delivery::queued(id, 7);
    let sent = queued
        .admit(7, 2000)
        .unwrap()
        .observed(Observation::Success, 2100)
        .unwrap();
    record["beforeState"] = serde_json::from_str(&codec::encode(&queued).unwrap()).unwrap();
    record["afterState"] = serde_json::from_str(&codec::encode(&sent).unwrap()).unwrap();
    record["before"] = json!("queued");
    record["after"] = json!("sent");
    record["attempt"] = json!(1);
    record["timestampMs"] = json!(2000);
    record["cause"] = json!("response");
    record["initiator"] = json!(format!("delivery:{id}"));
    assert!(matches!(
        audit::Record::decode(&record.to_string()),
        Err(OutboxError::Invalid)
    ));
    record["beforeState"] = Value::Null;
    record["before"] = json!("absent");
    record["cause"] = json!("source_checkpoint");
    record["initiator"] = json!(QueueInitiator::Live.label(f.registration.id));
    assert!(matches!(
        audit::Record::decode(&record.to_string()),
        Err(OutboxError::Invalid)
    ));
}

#[tokio::test]
async fn retained_audit_chain_correlates_checkpoint_target_and_current_state() {
    for corruption in [
        "missing_first",
        "missing_last",
        "foreign_target",
        "foreign_checkpoint",
        "current_state",
        "orphan",
        "source_context",
    ] {
        let mut f = Fixture::new().await;
        let id = f.store.queue(f.record(1)).await.unwrap();
        let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
        let sending = queued.admit(7, 2000).unwrap();
        f.store
            .transition(&queued, &sending, DeliveryCause::Admission, 2000)
            .await
            .unwrap();
        if corruption == "source_context" {
            assert_eq!(
                f.store.queue(replaced_source(f.record(2))).await,
                Err(OutboxError::ResetRequired)
            );
            let retry = sending.observed(Observation::Retryable, 2100).unwrap();
            f.store
                .transition(&sending, &retry, DeliveryCause::Response, 2100)
                .await
                .unwrap();
        }
        {
            let c = f.store.owner.connection.lock().unwrap();
            match corruption {
                "missing_first" => {
                    c.execute("DELETE FROM audit WHERE json_extract(record,'$.cause')='source_checkpoint'",[]).unwrap();
                }
                "missing_last" => {
                    c.execute(
                        "DELETE FROM audit WHERE json_extract(record,'$.cause')='admission'",
                        [],
                    )
                    .unwrap();
                }
                "foreign_target" => {
                    c.execute("UPDATE audit SET record=json_set(record,'$.principal','other-valid-principal') WHERE json_extract(record,'$.cause')='source_checkpoint'",[]).unwrap();
                }
                "foreign_checkpoint" => {
                    c.execute("UPDATE audit SET record=json_set(record,'$.position',2) WHERE json_extract(record,'$.cause')='source_checkpoint'",[]).unwrap();
                }
                "current_state" => {
                    c.execute(
                        "UPDATE deliveries SET state=?1",
                        [codec::encode(&queued.admit(7, 9999).unwrap()).unwrap()],
                    )
                    .unwrap();
                }
                "source_context" => {
                    c.execute("UPDATE audit SET record=json_set(record,'$.sourceStop',NULL) WHERE delivery IS NOT NULL AND json_extract(record,'$.cause')='source_changed'",[]).unwrap();
                }
                "orphan" => {
                    c.execute("DELETE FROM deliveries WHERE id=?1", [id.to_string()])
                        .unwrap();
                }
                _ => unreachable!(),
            }
        }
        let before = f.audits();
        assert!(
            matches!(reopen_store(&mut f).await, Err(OutboxError::Invalid)),
            "accepted {corruption}"
        );
        assert_eq!(f.audits(), before, "changed evidence for {corruption}");
    }
}

#[tokio::test]
async fn cursor_restore_and_live_duplicate_decode_the_whole_value() {
    for set in [
        "position='invalid'",
        "position=1.5",
        "position=-1",
        "reset_required='invalid'",
        "reset_required=2",
    ] {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
        f.store
            .transition(
                &queued,
                &queued.refused().unwrap(),
                DeliveryCause::Refused,
                2001,
            )
            .await
            .unwrap();
        {
            let c = f.store.owner.connection.lock().unwrap();
            c.execute_batch("PRAGMA ignore_check_constraints=ON")
                .unwrap();
            c.execute(&format!("UPDATE cursors SET {set}"), []).unwrap();
            c.execute_batch("PRAGMA ignore_check_constraints=OFF")
                .unwrap();
        }
        let before = f.audits();
        assert_eq!(
            f.store.queue(f.record(1)).await,
            Err(OutboxError::Invalid),
            "duplicate accepted {set}"
        );
        let result = reopen_store(&mut f).await;
        assert!(
            matches!(&result, Err(OutboxError::Invalid)),
            "wrong restore result for {set}: {:?}",
            result.as_ref().err()
        );
        assert_eq!(f.audits(), before);
    }
    let mut f = Fixture::new().await;
    let id = f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
    f.store
        .transition(
            &queued,
            &queued.refused().unwrap(),
            DeliveryCause::Refused,
            2001,
        )
        .await
        .unwrap();
    f.store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute("UPDATE cursors SET position=?1", [i64::MAX])
        .unwrap();
    let reopened = reopen_store(&mut f).await.unwrap();
    assert_eq!(reopened.queue(f.record(1)).await.unwrap(), id);
}

#[tokio::test]
async fn retired_historical_actor_survives_lost_publication_ack_and_restart() {
    let mut f = Fixture::new().await;
    f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER lose_retire_ack BEFORE UPDATE OF published ON audit WHEN json_extract(OLD.record,'$.cause')='drained' BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    assert_eq!(
        f.store
            .stop(
                f.registration.target.credential.clone(),
                StopReason::Revoked {
                    initiator: PrincipalId::new("original-other-revoker").unwrap()
                },
                2000
            )
            .await,
        Err(OutboxError::Conflict)
    );
    let (id, text): (String, String) = {
        let c = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM registrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        c.execute_batch("DROP TRIGGER lose_retire_ack").unwrap();
        c.query_row("SELECT id,record FROM audit WHERE published=0", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap()
    };
    let path = f.audit.join(format!("{id}.json"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    std::fs::remove_file(&path).unwrap();
    let reopened = reopen_store(&mut f).await.unwrap();
    assert!(reopened.due(3000).await.unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    let record: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(record["initiator"], "original-other-revoker");
    assert_eq!(record["owner"], "synthetic-owner");
    assert_eq!(record["stopAtMs"], 2000);
    assert_eq!(record["stopCause"], "credential_revoked");
}

#[tokio::test]
async fn registration_audit_lifecycle_and_actor_are_validated() {
    let f = Fixture::new().await;
    f.store
        .stop(
            f.registration.target.credential.clone(),
            StopReason::Revoked {
                initiator: PrincipalId::new("historical-revoker").unwrap(),
            },
            2000,
        )
        .await
        .unwrap();
    for original in f
        .audits()
        .into_iter()
        .filter(|r| r["kind"] == "webhook_registration")
    {
        audit::Record::decode(&original.to_string()).unwrap();
        for (field, value) in [
            ("principal", json!("")),
            ("credential", json!("")),
            ("organization", json!("")),
            ("owner", json!("")),
            ("before", json!("impossible")),
            ("after", json!("impossible")),
            ("atMs", json!(-1)),
            ("cause", json!("source_changed")),
            ("initiator", json!("")),
            ("unrecognized", json!("synthetic-secret")),
            ("stopAtMs", json!(-1)),
            ("stopAtMs", json!(u64::MAX)),
            ("stopCause", json!("credential_expired")),
        ] {
            let mut malformed = original.clone();
            malformed[field] = value;
            assert!(
                matches!(
                    audit::Record::decode(&malformed.to_string()),
                    Err(OutboxError::Invalid)
                ),
                "accepted {field} on {}",
                original["cause"]
            );
        }
    }
}

#[tokio::test]
async fn typed_delivery_actors_are_correlated_to_saved_cause() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    f.store.queue(f.record(2)).await.unwrap();
    let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    let retry = sending.observed(Observation::Retryable, 2100).unwrap();
    f.store
        .transition(&sending, &retry, DeliveryCause::Response, 2100)
        .await
        .unwrap();
    let sending_again = retry.admit(7, 4000).unwrap();
    f.store
        .transition(&retry, &sending_again, DeliveryCause::Admission, 4000)
        .await
        .unwrap();
    f.store.recover(5000).await.unwrap();
    f.store
        .stop(
            f.registration.target.credential.clone(),
            StopReason::Revoked {
                initiator: PrincipalId::new("original-revoker").unwrap(),
            },
            6000,
        )
        .await
        .unwrap();
    for original in f
        .audits()
        .into_iter()
        .filter(|r| r["kind"] == "webhook_delivery")
    {
        audit::Record::decode(&original.to_string()).unwrap();
        let mut malformed = original.clone();
        malformed["initiator"] = json!("");
        assert!(
            matches!(
                audit::Record::decode(&malformed.to_string()),
                Err(OutboxError::Invalid)
            ),
            "accepted empty actor for {}",
            original["cause"]
        );
        if original["cause"] != "credential_revoked" {
            malformed["initiator"] = json!("delivery:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
            assert!(
                matches!(
                    audit::Record::decode(&malformed.to_string()),
                    Err(OutboxError::Invalid)
                ),
                "accepted foreign actor for {}",
                original["cause"]
            );
        }
    }
}

#[tokio::test]
async fn cursor_audit_restores_its_transition_and_scope() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    assert_eq!(
        f.store.queue(replaced_source(f.record(2))).await,
        Err(OutboxError::ResetRequired)
    );
    let original = f
        .audits()
        .into_iter()
        .find(|r| r["kind"] == "webhook_cursor")
        .unwrap();
    audit::Record::decode(&original.to_string()).unwrap();
    for (field, value) in [
        ("before", json!("absent")),
        ("after", json!("following")),
        ("cause", json!("source_checkpoint")),
        (
            "initiator",
            json!("reader:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ),
        ("priorPosition", json!(-1)),
        ("priorPosition", json!(u64::MAX)),
        ("atMs", json!(-1)),
        ("priorScope.extra", json!("synthetic-secret")),
        (
            "priorScope.receiver",
            json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ),
        (
            "observedScope.receiver",
            json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ),
        ("observedScope", original["priorScope"].clone()),
    ] {
        let mut malformed = original.clone();
        if let Some((parent, key)) = field.split_once('.') {
            malformed[parent][key] = value;
        } else {
            malformed[field] = value;
        }
        assert!(
            matches!(
                audit::Record::decode(&malformed.to_string()),
                Err(OutboxError::Invalid)
            ),
            "accepted cursor {field}"
        );
    }
}

#[tokio::test]
async fn settled_source_context_must_match_the_enclosing_delivery() {
    for corruption in ["missing", "actor", "time"] {
        let mut f = Fixture::new().await;
        let id = f.store.queue(f.record(1)).await.unwrap();
        let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
        let sending = queued.admit(7, 2000).unwrap();
        f.store
            .transition(&queued, &sending, DeliveryCause::Admission, 2000)
            .await
            .unwrap();
        assert_eq!(
            f.store.queue(replaced_source(f.record(2))).await,
            Err(OutboxError::ResetRequired)
        );
        let sent = sending.observed(Observation::Success, 2100).unwrap();
        f.store
            .transition(&sending, &sent, DeliveryCause::Response, 2100)
            .await
            .unwrap();
        let (audit_id,text):(String,String)=f.store.owner.connection.lock().unwrap().query_row("SELECT id,record FROM audit WHERE delivery=?1 AND json_extract(record,'$.cause')='response'",[id.to_string()],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        let mut record: Value = serde_json::from_str(&text).unwrap();
        assert!(record["sourceStop"].is_object());
        match corruption {
            "missing" => record["sourceStop"] = Value::Null,
            "actor" => record["sourceStop"]["initiator"] = json!("restart"),
            "time" => record["sourceStop"]["at_ms"] = json!(0),
            _ => unreachable!(),
        }
        std::fs::remove_file(f.audit.join(format!("{audit_id}.json"))).unwrap();
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE audit SET record=?1,published=0 WHERE id=?2",
                params![record.to_string(), audit_id],
            )
            .unwrap();
        assert!(
            matches!(reopen_store(&mut f).await, Err(OutboxError::Invalid)),
            "accepted source context {corruption}"
        );
        assert!(!f.audit.join(format!("{audit_id}.json")).exists());
    }
}

#[tokio::test]
async fn sending_capture_can_precede_its_observation_audit_on_restore() {
    let mut f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    let replacement = replaced_source(f.record(2));
    let observed_at = replacement.observed_at_ms;
    assert_eq!(
        f.store.queue(replacement).await,
        Err(OutboxError::ResetRequired)
    );
    let reopened = reopen_store(&mut f).await.unwrap();
    reopened.recover(3000).await.unwrap();
    let unknown = f
        .audits()
        .into_iter()
        .find(|r| r["cause"] == "restart" && r["after"] == "unknown")
        .unwrap();
    assert_eq!(unknown["sourceStop"]["at_ms"], observed_at);
    assert_eq!(unknown["sourceStop"]["initiator"], "live");
    assert!(reopened.due(4000).await.unwrap().is_empty());
}

#[tokio::test]
async fn owed_credential_stop_evidence_matches_original_registration_facts() {
    for corruption in [
        "valid",
        "actor_only",
        "cause_context",
        "missing_context",
        "unknown_context_field",
        "actor_and_context",
        "time",
        "active_false_cause",
    ] {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        f.store.queue(f.record(2)).await.unwrap();
        let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
        let sending = queued.admit(7, 2000).unwrap();
        f.store
            .transition(&queued, &sending, DeliveryCause::Admission, 2000)
            .await
            .unwrap();
        let audit_cause = if corruption == "active_false_cause" {
            let second = f.store.due(2000).await.unwrap().remove(0).delivery;
            f.store
                .transition(
                    &second,
                    &second.refused().unwrap(),
                    DeliveryCause::Refused,
                    2001,
                )
                .await
                .unwrap();
            "admission_refused"
        } else {
            let held = f.audit.with_extension("held");
            std::fs::rename(&f.audit, &held).unwrap();
            assert_eq!(
                f.store
                    .stop(
                        f.registration.target.credential.clone(),
                        StopReason::Revoked {
                            initiator: PrincipalId::new("original-revoker").unwrap()
                        },
                        2001
                    )
                    .await,
                Err(OutboxError::Unavailable)
            );
            std::fs::rename(&held, &f.audit).unwrap();
            "credential_revoked"
        };
        let (id,text):(String,String)=f.store.owner.connection.lock().unwrap().query_row("SELECT id,record FROM audit WHERE delivery IS NOT NULL AND json_extract(record,'$.cause')=?1",[audit_cause],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        let mut record: Value = serde_json::from_str(&text).unwrap();
        match corruption {
            "valid" => {}
            "actor_only" => record["initiator"] = json!("unrelated-revoker"),
            "cause_context" => record["cause"] = json!("credential_expired"),
            "missing_context" => record["credentialStop"] = Value::Null,
            "unknown_context_field" => {
                record["credentialStop"]["extra"] = json!("synthetic-secret")
            }
            "actor_and_context" => {
                record["initiator"] = json!("unrelated-revoker");
                record["credentialStop"]["initiator"] = json!("unrelated-revoker");
            }
            "time" => record["credentialStop"]["atMs"] = json!(1000),
            "active_false_cause" => {
                record["cause"] = json!("credential_revoked");
                record["initiator"] = json!("unrelated-revoker");
                record["credentialStop"] = json!({"cause":"credential_revoked","initiator":"unrelated-revoker","atMs":2001});
            }
            _ => unreachable!(),
        }
        let path = f.audit.join(format!("{id}.json"));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE audit SET record=?1,published=0 WHERE id=?2",
                params![record.to_string(), id],
            )
            .unwrap();
        let result = reopen_store(&mut f).await;
        if corruption == "valid" {
            let reopened = result.unwrap();
            assert!(reopened.due(3000).await.unwrap().is_empty());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            let actual: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(actual["credentialStop"]["initiator"], "original-revoker");
            assert_eq!(actual["credentialStop"]["atMs"], 2001);
        } else {
            assert!(
                matches!(result, Err(OutboxError::Invalid)),
                "accepted stop {corruption}"
            );
            assert!(!path.exists());
        }
    }
}

/// Corrupt only the private fixture's saved evidence and make publication owed.
fn edit_evidence(f: &Fixture, kind: &str, cause: &str, edit: impl FnOnce(&mut Value)) -> String {
    let connection = f.store.owner.connection.lock().unwrap();
    let (id, text): (String, String) = connection.query_row(
        "SELECT id,record FROM audit WHERE json_extract(record,'$.kind')=?1 AND json_extract(record,'$.cause')=?2 ORDER BY rowid DESC LIMIT 1",
        params![kind,cause], |r| Ok((r.get(0)?,r.get(1)?)),
    ).unwrap();
    let mut record: Value = serde_json::from_str(&text).unwrap();
    edit(&mut record);
    connection
        .execute(
            "UPDATE audit SET record=?1,published=0 WHERE id=?2",
            params![record.to_string(), id],
        )
        .unwrap();
    let path = f.audit.join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(path).unwrap();
    }
    id
}

async fn rejects_evidence_before_live_or_restored_publication(f: &mut Fixture, id: &str) {
    let path = f.audit.join(format!("{id}.json"));
    assert_eq!(f.store.due(3000).await.err(), Some(OutboxError::Invalid));
    assert!(!path.exists());
    assert_eq!(reopen_store(f).await.err(), Some(OutboxError::Invalid));
    assert!(!path.exists());
}

#[tokio::test]
async fn registration_issue_evidence_matches_retained_target() {
    for field in ["credential", "principal", "organization", "owner", "atMs"] {
        let mut f = Fixture::new().await;
        let id = edit_evidence(&f, "webhook_registration", "credential_issue", |r| {
            r[field] = if field == "atMs" {
                json!(1001)
            } else {
                json!("another-valid-target")
            };
            if field == "owner" {
                r["initiator"] = r["owner"].clone();
            }
        });
        rejects_evidence_before_live_or_restored_publication(&mut f, &id).await;
    }
}

#[tokio::test]
async fn registration_stop_evidence_matches_original_stop_and_target() {
    for field in ["initiator", "cause", "atMs", "principal", "owner"] {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        let q = f.store.due(2000).await.unwrap().remove(0).delivery;
        let sending = q.admit(7, 2000).unwrap();
        f.store
            .transition(&q, &sending, DeliveryCause::Admission, 2000)
            .await
            .unwrap();
        let held = f.audit.with_extension("held");
        std::fs::rename(&f.audit, &held).unwrap();
        assert_eq!(
            f.store
                .stop(
                    f.registration.target.credential.clone(),
                    StopReason::Revoked {
                        initiator: PrincipalId::new("original-revoker").unwrap()
                    },
                    2001
                )
                .await,
            Err(OutboxError::Unavailable)
        );
        std::fs::rename(&held, &f.audit).unwrap();
        let id = edit_evidence(
            &f,
            "webhook_registration",
            "credential_revoked",
            |r| match field {
                "atMs" => r[field] = json!(2002),
                "cause" => {
                    r[field] = json!("credential_expired");
                    r["initiator"] = json!(format!("delivery:{}", Uuid::new_v4()));
                }
                _ => r[field] = json!("replacement-valid-principal"),
            },
        );
        rejects_evidence_before_live_or_restored_publication(&mut f, &id).await;
    }
}

#[tokio::test]
async fn retirement_evidence_uses_retained_original_facts_after_private_row_deletion() {
    for field in [
        "valid",
        "initiator",
        "principal",
        "stopCause",
        "stopAtMs",
        "atMs",
        "missing_facts",
        "changed_facts",
        "invalid_facts",
        "wrong_audit_identity",
        "missing_retirement_record",
        "replaced_retirement_record",
        "duplicate_retirement_record",
        "excess_facts",
    ] {
        let mut f = Fixture::new().await;
        f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER hold_retirement_ack BEFORE UPDATE OF published ON audit WHEN json_extract(OLD.record,'$.cause')='drained' BEGIN SELECT RAISE(IGNORE); END;").unwrap();
        assert_eq!(
            f.store
                .stop(
                    f.registration.target.credential.clone(),
                    StopReason::Revoked {
                        initiator: PrincipalId::new("original-other-revoker").unwrap()
                    },
                    2000
                )
                .await,
            Err(OutboxError::Conflict)
        );
        {
            let c = f.store.owner.connection.lock().unwrap();
            c.execute_batch("DROP TRIGGER hold_retirement_ack").unwrap();
            assert_eq!(
                c.query_row("SELECT count(*) FROM registrations", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                c.query_row("SELECT count(*) FROM retirements", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
        let id = edit_evidence(&f, "webhook_registration", "drained", |r| match field {
            "initiator" | "principal" => r[field] = json!("substituted-valid-principal"),
            "stopCause" => {
                r[field] = json!("credential_expired");
                r["initiator"] = json!(format!("delivery:{}", Uuid::new_v4()));
            }
            "stopAtMs" | "atMs" => r[field] = json!(2002),
            _ => {}
        });
        {
            let c = f.store.owner.connection.lock().unwrap();
            match field {
                "missing_facts" => {
                    c.execute("DELETE FROM retirements", []).unwrap();
                }
                "changed_facts" => {
                    c.execute(
                        "UPDATE retirements SET stop_initiator='changed-original'",
                        [],
                    )
                    .unwrap();
                }
                "invalid_facts" => {
                    c.execute("UPDATE retirements SET retired_at='invalid'", [])
                        .unwrap();
                }
                "wrong_audit_identity" => {
                    c.execute("UPDATE retirements SET audit=(SELECT id FROM audit WHERE json_extract(record,'$.cause')='credential_revoked')", []).unwrap();
                }
                "replaced_retirement_record" => {
                    c.execute("UPDATE retirements SET audit=(SELECT id FROM audit WHERE json_extract(record,'$.cause')='credential_revoked')", []).unwrap();
                    c.execute("DELETE FROM audit WHERE id=?1", [&id]).unwrap();
                }
                "duplicate_retirement_record" => {
                    let text: String = c
                        .query_row("SELECT record FROM audit WHERE id=?1", [&id], |r| r.get(0))
                        .unwrap();
                    let mut record: Value = serde_json::from_str(&text).unwrap();
                    let another = Uuid::new_v4().to_string();
                    record["recordId"] = json!(another);
                    c.execute(
                        "INSERT INTO audit(id,registration,record) VALUES(?1,?2,?3)",
                        params![another, f.registration.id.to_string(), record.to_string()],
                    )
                    .unwrap();
                }
                "excess_facts" => {
                    let text: String = c
                        .query_row("SELECT record FROM audit WHERE id=?1", [&id], |r| r.get(0))
                        .unwrap();
                    for _ in 0..crate::webhooks::application::MAX_REGISTRATIONS {
                        let another = Uuid::new_v4().to_string();
                        let registration = Uuid::new_v4().to_string();
                        let mut record: Value = serde_json::from_str(&text).unwrap();
                        record["recordId"] = json!(another);
                        record["registration"] = json!(registration);
                        c.execute(
                            "INSERT INTO audit(id,registration,record) VALUES(?1,?2,?3)",
                            params![another, registration, record.to_string()],
                        )
                        .unwrap();
                        c.execute("INSERT INTO retirements(id,audit,credential,principal,organization,owner,issued_at,stop_cause,stop_initiator,stop_at,retired_at) SELECT ?1,?2,credential,principal,organization,owner,issued_at,stop_cause,stop_initiator,stop_at,retired_at FROM retirements WHERE id=?3",params![registration,another,f.registration.id.to_string()]).unwrap();
                    }
                }
                "missing_retirement_record" => {
                    // Model a damaged private file; the production FK remains enabled.
                    c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
                    c.execute("DELETE FROM audit WHERE id=?1", [&id]).unwrap();
                    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
                }
                _ => {}
            }
        }
        if field == "valid" {
            // The actual FK refuses deletion of the owed retirement identity.
            assert!(f
                .store
                .owner
                .connection
                .lock()
                .unwrap()
                .execute("DELETE FROM audit WHERE id=?1", [&id])
                .is_err());
            let reopened = reopen_store(&mut f).await.unwrap();
            assert!(reopened.due(3000).await.unwrap().is_empty());
            let record: Value = serde_json::from_str(
                &std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(record["initiator"], "original-other-revoker");
            assert_eq!(
                reopened
                    .owner
                    .connection
                    .lock()
                    .unwrap()
                    .query_row("SELECT count(*) FROM retirements", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        } else {
            rejects_evidence_before_live_or_restored_publication(&mut f, &id).await;
        }
    }
}

#[tokio::test]
async fn cursor_evidence_matches_complete_original_reset_observation() {
    for field in [
        "valid",
        "priorPosition",
        "priorScope",
        "observedScope",
        "initiator",
        "atMs",
        "missing_observation",
        "changed_observation",
    ] {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        let q = f.store.due(2000).await.unwrap().remove(0).delivery;
        let sending = q.admit(7, 2000).unwrap();
        f.store
            .transition(&q, &sending, DeliveryCause::Admission, 2000)
            .await
            .unwrap();
        let held = f.audit.with_extension("held");
        std::fs::rename(&f.audit, &held).unwrap();
        assert_eq!(
            f.store.queue(replaced_source(f.record(2))).await,
            Err(OutboxError::Unavailable)
        );
        std::fs::rename(&held, &f.audit).unwrap();
        let id = edit_evidence(&f, "webhook_cursor", "source_changed", |r| match field {
            "priorPosition" => r[field] = json!(999),
            "priorScope" | "observedScope" => {
                r[field]["incarnation"] = json!("other-valid-incarnation")
            }
            "initiator" => r[field] = json!("restart"),
            "atMs" => r[field] = json!(2002),
            _ => {}
        });
        {
            let c = f.store.owner.connection.lock().unwrap();
            match field {
                "missing_observation" => {
                    c.execute("UPDATE cursors SET source_reset=NULL", [])
                        .unwrap();
                }
                "changed_observation" => {
                    let old: String = c
                        .query_row("SELECT source_reset FROM cursors", [], |r| r.get(0))
                        .unwrap();
                    let original = source_reset::read(&c, Uuid::parse_str(&old).unwrap())
                        .unwrap()
                        .unwrap();
                    let other = source_reset::Fact {
                        id: Uuid::new_v4(),
                        stop: codec::SourceStop {
                            initiator: QueueInitiator::Restart,
                            at_ms: 2001,
                        },
                        ..original
                    };
                    c.execute(
                        "INSERT INTO source_reset_facts(id,registration,fact) VALUES(?1,?2,?3)",
                        params![
                            other.id.to_string(),
                            other.registration.to_string(),
                            other.encode().unwrap()
                        ],
                    )
                    .unwrap();
                    c.execute("UPDATE cursors SET source_reset=?1", [other.id.to_string()])
                        .unwrap();
                }
                _ => {}
            }
        }
        if field == "valid" {
            let reopened = reopen_store(&mut f).await.unwrap();
            assert!(reopened.due(3000).await.unwrap().is_empty());
            assert!(f.audit.join(format!("{id}.json")).exists());
            reopened.recover(3100).await.unwrap();
        } else {
            rejects_evidence_before_live_or_restored_publication(&mut f, &id).await;
        }
    }
}

#[tokio::test]
async fn complete_owed_batch_is_checked_before_first_publication() {
    let f = Fixture::new().await;
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store.queue(f.record(1)).await.err(),
        Some(OutboxError::Unavailable)
    );
    std::fs::rename(&held, &f.audit).unwrap();
    let first: String = f
        .store
        .owner
        .connection
        .lock()
        .unwrap()
        .query_row("SELECT id FROM audit WHERE published=0", [], |r| r.get(0))
        .unwrap();
    let mut other = f.registration.clone();
    other.target.principal = PrincipalId::new("other-valid-principal").unwrap();
    {
        let mut c = f.store.owner.connection.lock().unwrap();
        let transaction = c.transaction().unwrap();
        publication::append(
            &transaction,
            audit::Record::registration(&other, audit::RegistrationChange::Issue, 1000),
        )
        .unwrap();
        transaction.commit().unwrap();
    }
    assert_eq!(f.store.due(3000).await.err(), Some(OutboxError::Invalid));
    assert!(!f.audit.join(format!("{first}.json")).exists());
    assert_eq!(
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM audit WHERE published=0", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn delivery_evidence_matches_enclosing_facts_before_publication() {
    for field in ["principal", "position", "scope"] {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        let id = edit_evidence(
            &f,
            "webhook_delivery",
            "source_checkpoint",
            |r| match field {
                "position" => r[field] = json!(999),
                "scope" => r[field]["origin"] = json!("another-valid-origin"),
                _ => r[field] = json!("another-valid-principal"),
            },
        );
        rejects_evidence_before_live_or_restored_publication(&mut f, &id).await;
    }
}

#[tokio::test]
async fn reset_facts_survive_audit_trim_and_decode_completely() {
    for change in [
        "valid",
        "source_reset=NULL",
        "reset_required=0",
        "source_reset='invalid'",
        "source_reset='00000000-0000-0000-0000-000000000001'",
    ] {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        assert_eq!(
            f.store.queue(replaced_source(f.record(2))).await,
            Err(OutboxError::ResetRequired)
        );
        assert!(f.store.due(3000).await.unwrap().is_empty());
        {
            let c = f.store.owner.connection.lock().unwrap();
            assert_eq!(c.query_row("SELECT count(*) FROM audit WHERE json_extract(record,'$.kind')='webhook_cursor'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
            if change != "valid" {
                // Isolated malformed-reference fixture; production FK stays enabled.
                c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
                c.execute(&format!("UPDATE cursors SET {change}"), [])
                    .unwrap();
                c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            }
        }
        let reopened = reopen_store(&mut f).await;
        if change == "valid" {
            assert_eq!(
                reopened.unwrap().queue(replaced_source(f.record(2))).await,
                Err(OutboxError::ResetRequired)
            );
        } else {
            assert_eq!(reopened.err(), Some(OutboxError::Invalid));
        }
    }
}

#[tokio::test]
async fn live_owed_delivery_history_matches_current_before_publication() {
    let mut f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let id = edit_evidence(&f, "webhook_delivery", "source_checkpoint", |r| {
        r["beforeState"] = r["afterState"].clone();
        r["before"] = json!("queued");
        r["afterState"]["phase"] = json!("not_sent");
        r["after"] = json!("not_sent");
        r["cause"] = json!("admission_refused");
        r["initiator"] = json!(format!("delivery:{}", r["deliveryId"].as_str().unwrap()));
    });
    {
        let c = f.store.owner.connection.lock().unwrap();
        let text: String = c
            .query_row("SELECT record FROM audit WHERE id=?1", [&id], |r| r.get(0))
            .unwrap();
        // Locally valid values still describe an impossible owning history.
        audit::Record::decode(&text).unwrap();
    }
    rejects_evidence_before_live_or_restored_publication(&mut f, &id).await;
}

#[path = "outbox/source_reset.rs"]
mod source_reset_tests;

#[path = "outbox/registration.rs"]
mod registration_tests;

#[path = "outbox/cleanup.rs"]
mod cleanup_tests;

#[path = "outbox/lifecycle_context.rs"]
mod lifecycle_context_tests;

#[path = "outbox/issuance_context.rs"]
mod issuance_context_tests;
