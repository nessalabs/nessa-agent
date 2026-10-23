//! Phase/ack/work contradictions reject before publication, including after trim.
use super::*;

fn saved(f: &Fixture) -> Vec<String> {
    let c = f.store.owner.connection.lock().unwrap();
    [
        "SELECT json_group_array(json_array(id,active,stop)) FROM registrations",
        "SELECT json_group_array(json_array(id,state)) FROM deliveries",
        "SELECT json_group_array(json_array(id,record,published)) FROM audit",
        "SELECT json_group_array(json_array(id,audit)) FROM retirements",
        "SELECT json_group_array(json_array(registration,conversation,scope,position,reset_required)) FROM cursors",
    ].iter().map(|sql| c.query_row(sql, [], |r| r.get(0)).unwrap()).collect()
}

fn cold_repair(f: &mut Fixture, repair: impl FnOnce(&Connection)) {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let temporary = SqlOutbox::open(
        &private.join("outbox.db"),
        private.join("audit"),
        f.clock.clone(),
    )
    .unwrap();
    drop(std::mem::replace(&mut f.store, temporary));
    let files = std::fs::read_dir(&f.audit).unwrap().count();
    assert!(matches!(
        SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
        Err(OutboxError::Invalid)
    ));
    assert_eq!(std::fs::read_dir(&f.audit).unwrap().count(), files);
    let connection = Connection::open(&f.path).unwrap();
    repair(&connection);
    drop(connection);
    let original = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
    drop(std::mem::replace(&mut f.store, original));
}

#[tokio::test]
async fn owed_observation_cannot_restore_as_stopped_before_its_acknowledgement() {
    let mut f = Fixture::new().await;
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store
            .observe_expiry(
                f.registration.target.credential.clone(),
                ExpiryObserver::Restart,
                2001
            )
            .await,
        Err(OutboxError::Unavailable)
    );
    f.store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute("UPDATE registrations SET active=0", [])
        .unwrap();
    let before = saved(&f);
    assert_eq!(f.store.due(3000).await.err(), Some(OutboxError::Invalid));
    assert_eq!(saved(&f), before);
    assert!(!f.audit.exists());
    std::fs::rename(held, &f.audit).unwrap();
    cold_repair(&mut f, |c| {
        c.execute("UPDATE registrations SET active=1", []).unwrap();
    });
    f.store.due(3000).await.unwrap();
    f.store
        .stop(
            f.registration.target.credential.clone(),
            StopReason::Expired {
                observed_by: ExpiryObserver::Restart,
            },
            3001,
        )
        .await
        .unwrap();
    assert!(f
        .audits()
        .iter()
        .any(|v| v["kind"] == "webhook_registration" && v["cause"] == "credential_expired"));
}

#[tokio::test]
async fn stopped_cannot_retain_queued_work_after_observation_audit_was_trimmed() {
    let mut f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    f.store
        .observe_expiry(
            f.registration.target.credential.clone(),
            ExpiryObserver::Restart,
            2001,
        )
        .await
        .unwrap();
    {
        let c = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM audit WHERE delivery IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        c.execute("UPDATE registrations SET active=0", []).unwrap();
    }
    let before = saved(&f);
    let files = f.audits().len();
    assert_eq!(f.store.due(3000).await.err(), Some(OutboxError::Invalid));
    assert_eq!(saved(&f), before);
    assert_eq!(f.audits().len(), files);
    cold_repair(&mut f, |c| {
        c.execute("UPDATE registrations SET active=1", []).unwrap();
    });
    assert_eq!(f.store.due(3000).await.unwrap().len(), 1);
    f.store
        .stop(
            f.registration.target.credential.clone(),
            StopReason::Expired {
                observed_by: ExpiryObserver::Restart,
            },
            3001,
        )
        .await
        .unwrap();
    assert!(f.store.due(4000).await.unwrap().is_empty());
    assert!(f
        .audits()
        .iter()
        .any(|v| v["kind"] == "webhook_registration" && v["cause"] == "credential_expired"));
}

#[tokio::test]
async fn retired_cannot_owe_its_prerequisite_observation_again() {
    let mut f = Fixture::new().await;
    f.store
        .observe_expiry(
            f.registration.target.credential.clone(),
            ExpiryObserver::Restart,
            2001,
        )
        .await
        .unwrap();
    let observation = std::fs::read_dir(&f.audit)
        .unwrap()
        .map(|p| std::fs::read_to_string(p.unwrap().path()).unwrap())
        .find(|s| {
            serde_json::from_str::<Value>(s).unwrap()["cause"] == "credential_expiry_observed"
        })
        .unwrap();
    let value: Value = serde_json::from_str(&observation).unwrap();
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store
            .stop(
                f.registration.target.credential.clone(),
                StopReason::Expired {
                    observed_by: ExpiryObserver::Restart
                },
                3000
            )
            .await,
        Err(OutboxError::Unavailable)
    );
    let id = value["recordId"].as_str().unwrap().to_owned();
    {
        let c = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM registrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        c.execute(
            "INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,0)",
            params![id, f.registration.id.to_string(), observation],
        )
        .unwrap();
    }
    let before = saved(&f);
    assert_eq!(f.store.due(4000).await.err(), Some(OutboxError::Invalid));
    assert_eq!(saved(&f), before);
    assert!(!f.audit.exists());
    std::fs::rename(held, &f.audit).unwrap();
    cold_repair(&mut f, |c| {
        c.execute("DELETE FROM audit WHERE id=?1", [&id]).unwrap();
    });
    assert!(f.store.due(4000).await.unwrap().is_empty());
}

#[tokio::test]
async fn late_retry_after_genuine_stop_and_lost_response_ack_remains_valid() {
    let mut f = Fixture::new().await;
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
            StopReason::Expired {
                observed_by: ExpiryObserver::Restart,
            },
            2001,
        )
        .await
        .unwrap();
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    let retry = sending.observed(Observation::Retryable, 2002).unwrap();
    assert_eq!(
        f.store
            .transition(&sending, &retry, DeliveryCause::Response, 2002)
            .await,
        Err(OutboxError::Unavailable)
    );
    std::fs::rename(held, &f.audit).unwrap();
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let temporary = SqlOutbox::open(
        &private.join("outbox.db"),
        private.join("audit"),
        f.clock.clone(),
    )
    .unwrap();
    drop(std::mem::replace(&mut f.store, temporary));
    f.store = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
    assert!(f.store.due(4000).await.unwrap().is_empty());
    let records = f.audits();
    assert!(records
        .iter()
        .any(|v| v["cause"] == "response" && v["after"] == "retry"));
    assert!(records
        .iter()
        .filter(|v| v["cause"] == "credential_expired")
        .all(|v| v["initiator"] == "restart"));
}

#[tokio::test]
async fn stopped_queue_admission_refuses_before_cursor_or_delivery_commit() {
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
                initiator: nessa_auth::domain::Initiator::Principal(
                    PrincipalId::new("owner").unwrap(),
                ),
                cause: nessa_auth::domain::RevocationCause::Explicit,
            },
            2001,
        )
        .await
        .unwrap();
    let before = saved(&f);
    assert_eq!(f.store.queue(f.record(2)).await, Err(OutboxError::Conflict));
    assert_eq!(saved(&f), before);
}
