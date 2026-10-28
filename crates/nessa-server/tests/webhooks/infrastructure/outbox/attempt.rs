//! Native exclusion spans admission through consumed acknowledgement.
use super::*;
use tokio::time::{timeout, Duration, Instant};
const WAIT: Duration = Duration::from_secs(10);

async fn queued(f: &Fixture) -> Delivery {
    f.store.queue(f.record(1)).await.unwrap();
    f.store.due(10_000).await.unwrap().pop().unwrap().delivery
}
async fn stopped(f: &Fixture) -> Result<(), OutboxError> {
    f.store
        .stop_credential(
            f.registration.target.credential.clone(),
            RegistrationStop::expired(ExpiryObserver::Restart, 10_002),
        )
        .await
}
async fn all_workers_joined(f: &Fixture) {
    timeout(WAIT, async {
        while f.store.owner.capacity.available_permits() != MAX_OUTBOX_WORKERS {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn pin_keeps_response_proof_through_outage_publication_and_delayed_ack_consumption() {
    let f = Fixture::new().await;
    let before = queued(&f).await;
    let attempt = f
        .store
        .delivery_attempt(f.registration.id, before.clone(), None);
    let sending = before.admit(7, 10_000).unwrap();
    f.store
        .admit_before(&attempt, &sending, 10_000, Instant::now() + WAIT)
        .await
        .unwrap();
    let response = sending.observed(Observation::Success, 10_001).unwrap();
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store
            .settle_response(&attempt, &sending, &response, 10_001)
            .await,
        Err(OutboxError::Unavailable)
    );
    let files = std::fs::read_dir(&held).unwrap().count();
    assert_eq!(stopped(&f).await, Err(OutboxError::Unavailable));
    assert_eq!(std::fs::read_dir(&held).unwrap().count(), files);
    std::fs::rename(&held, &f.audit).unwrap();
    f.store
        .settle_response(&attempt, &sending, &response, 10_001)
        .await
        .unwrap();
    // Successful publication is not yet consumed acknowledgement. Neither a
    // sibling nor another identical settlement may erase its receipt.
    assert_eq!(stopped(&f).await, Err(OutboxError::Unavailable));
    f.store
        .settle_response(&attempt, &sending, &response, 10_001)
        .await
        .unwrap();
    let c = Connection::open(&f.path).unwrap();
    let count: i64 = c
        .query_row(
            "SELECT count(*) FROM deliveries WHERE id=?1",
            [before.id().to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        f.audits()
            .iter()
            .filter(|a| a["cause"] == "response")
            .count(),
        1
    );
    drop(c);
    f.store.acknowledge_attempt(&attempt).unwrap();
    stopped(&f).await.unwrap();
    let c = Connection::open(&f.path).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM deliveries", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn stop_before_pin_prevents_admission_and_cold_open_cannot_bypass_live_pin_owner() {
    let f = Fixture::new().await;
    let before = queued(&f).await;
    let attempt = f
        .store
        .delivery_attempt(f.registration.id, before.clone(), None);
    stopped(&f).await.unwrap();
    let sending = before.admit(7, 10_000).unwrap();
    assert_eq!(
        f.store
            .admit_before(&attempt, &sending, 10_000, Instant::now() + WAIT)
            .await,
        Err(OutboxError::Conflict)
    );
    // An uninstalled attempt may reconcile absence only through actual retirement.
    f.store
        .settle_unposted(&attempt, &sending, false, 10_003)
        .await
        .unwrap();
    f.store.acknowledge_attempt(&attempt).unwrap();
    assert!(SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).is_err());
    assert_eq!(
        f.audits()
            .iter()
            .filter(|a| a["cause"] == "admission")
            .count(),
        0
    );
}

#[tokio::test]
async fn cleanup_queued_before_pin_rechecks_exclusion_under_connection_mutex() {
    let f = Fixture::new().await;
    let before = queued(&f).await;
    let attempt = f
        .store
        .delivery_attempt(f.registration.id, before.clone(), None);
    let sending = before.admit(7, 10_000).unwrap();
    let pause = f.store.pause_pin();
    let mut admission =
        Box::pin(
            f.store
                .admit_before(&attempt, &sending, 10_000, Instant::now() + WAIT),
        );
    tokio::select! { _ = pause.entered() => {}, result = &mut admission => panic!("admission ended before pin barrier: {result:?}") }
    let mut stop = Box::pin(stopped(&f));
    let enqueued = async {
        while f.store.owner.capacity.available_permits() != MAX_OUTBOX_WORKERS - 2 {
            tokio::task::yield_now().await;
        }
    };
    tokio::select! { result = timeout(WAIT, enqueued) => { result.unwrap(); }, result = &mut stop => panic!("stop ended before queue barrier: {result:?}") }
    pause.release();
    admission.await.unwrap();
    assert_eq!(stop.await, Err(OutboxError::Unavailable));
    let c = Connection::open(&f.path).unwrap();
    assert!(
        c.query_row(
            "SELECT stop IS NULL FROM registrations WHERE id=?1",
            [f.registration.id.to_string()],
            |r| r.get::<_, bool>(0)
        )
        .unwrap(),
        "queued expiry must refuse before changing lifecycle"
    );
    drop(c);
    assert_eq!(
        f.audits()
            .iter()
            .filter(|a| a["cause"] == "credential_expiry_observed")
            .count(),
        0
    );
    f.store
        .settle_unposted(&attempt, &sending, false, 10_001)
        .await
        .unwrap();
    // Unposted reconciliation also retains proof until its ACK is consumed.
    assert_eq!(stopped(&f).await, Err(OutboxError::Unavailable));
    f.store
        .settle_unposted(&attempt, &sending, false, 10_001)
        .await
        .unwrap();
    f.store.acknowledge_attempt(&attempt).unwrap();
    stopped(&f).await.unwrap();
}

#[tokio::test]
async fn lost_admission_observer_keeps_pin_until_late_physical_work_and_unposted_ack_join() {
    let f = Fixture::new().await;
    let before = queued(&f).await;
    let attempt = f
        .store
        .delivery_attempt(f.registration.id, before.clone(), None);
    let sending = before.admit(7, 10_000).unwrap();
    let pause = f.store.pause_admission(true);
    let mut admission =
        Box::pin(
            f.store
                .admit_before(&attempt, &sending, 10_000, Instant::now() + WAIT),
        );
    tokio::select! { _ = pause.entered() => {}, result = &mut admission => panic!("admission ended before committed barrier: {result:?}") }
    drop(admission); // Drop the result channel, not the admitted physical work.
    let stop = timeout(WAIT, stopped(&f)).await;
    let cold = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).is_err();
    pause.release();
    all_workers_joined(&f).await;
    assert_eq!(
        stop,
        Ok(Err(OutboxError::Unavailable)),
        "ordinary refusal must not wait on the held native connection"
    );
    assert!(cold);
    assert_eq!(
        f.store.clone().recover_live(10_001).await,
        Err(OutboxError::Unavailable)
    );
    f.store
        .settle_unposted(&attempt, &sending, false, 10_001)
        .await
        .unwrap();
    f.store.acknowledge_attempt(&attempt).unwrap();
    stopped(&f).await.unwrap();
    assert_eq!(
        f.audits()
            .iter()
            .filter(|a| a["cause"] == "response")
            .count(),
        0
    );
}

#[tokio::test]
async fn unposted_observation_pins_when_admission_capacity_refused_before_installation() {
    let f = Fixture::new().await;
    let before = queued(&f).await;
    let attempt = f
        .store
        .delivery_attempt(f.registration.id, before.clone(), None);
    let sending = before.admit(7, 10_000).unwrap();
    let occupied = f
        .store
        .owner
        .capacity
        .clone()
        .acquire_many_owned(MAX_OUTBOX_WORKERS as u32)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .admit_before(&attempt, &sending, 10_000, Instant::now() + WAIT)
            .await,
        Err(OutboxError::Capacity)
    );
    drop(occupied);
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store
            .settle_unposted(&attempt, &sending, false, 10_001)
            .await,
        Err(OutboxError::Unavailable)
    );
    assert_eq!(stopped(&f).await, Err(OutboxError::Unavailable));
    std::fs::rename(&held, &f.audit).unwrap();
    f.store
        .settle_unposted(&attempt, &sending, false, 10_001)
        .await
        .unwrap();
    f.store.acknowledge_attempt(&attempt).unwrap();
    stopped(&f).await.unwrap();
    assert_eq!(
        f.audits()
            .iter()
            .filter(|a| a["cause"] == "admission")
            .count(),
        0
    );
}

#[tokio::test]
async fn due_rotation_advances_to_a_healthy_registration_before_repeating_refused_work() {
    let f = Fixture::new().await;
    for position in 1..=5 {
        f.store.queue(f.record(position)).await.unwrap();
    }
    let mut sibling = f.registration.clone();
    sibling.id = Uuid::new_v4();
    sibling.target.credential = CredentialId::new("synthetic-sibling-key").unwrap();
    sibling.target.principal = PrincipalId::new("synthetic-sibling").unwrap();
    f.store.install(sibling.clone(), 1000).await.unwrap();
    let mut record = f.record(1);
    record.registration = sibling.id;
    record.scope = Scope::new(
        Id::new(sibling.id.to_string()).unwrap(),
        record.scope.origin().clone(),
        record.scope.stream().clone(),
        record.scope.incarnation().clone(),
        record.scope.schema().clone(),
        record.scope.access_epoch().clone(),
    );
    record.expected_source = Some(record.scope.clone());
    f.store.queue(record).await.unwrap();
    let next = f
        .store
        .due_after(10_000, Some(f.registration.id))
        .await
        .unwrap();
    assert_eq!(next[0].registration.id, sibling.id);
    let next = f.store.due_after(10_000, Some(sibling.id)).await.unwrap();
    assert_eq!(next[0].registration.id, f.registration.id);
    assert_eq!(
        f.audits()
            .iter()
            .filter(|a| a["cause"] == "admission")
            .count(),
        0
    );
}

#[tokio::test]
async fn attempt_alone_retains_namespace_lock_after_all_store_handles_are_dropped() {
    let f = Fixture::new().await;
    let before = queued(&f).await;
    let attempt = f
        .store
        .delivery_attempt(f.registration.id, before.clone(), None);
    let sending = before.admit(7, 10_000).unwrap();
    f.store
        .admit_before(&attempt, &sending, 10_000, Instant::now() + WAIT)
        .await
        .unwrap();
    let Fixture {
        _root,
        path,
        audit,
        store,
        clock,
        ..
    } = f;
    drop(store);
    assert!(
        SqlOutbox::open(&path, audit.clone(), clock.clone()).is_err(),
        "the retained attempt itself keeps the native owner locked"
    );
    let handle = SqlOutbox {
        owner: attempt.owner.clone(),
    };
    handle
        .settle_unposted(&attempt, &sending, false, 10_001)
        .await
        .unwrap();
    handle.acknowledge_attempt(&attempt).unwrap();
    drop(handle);
    drop(attempt);
    let cold = SqlOutbox::open(&path, audit, clock).unwrap();
    cold.recover(10_002).await.unwrap();
    let c = Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT phase FROM deliveries WHERE id=?1",
            [sending.id().to_string()],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "unknown"
    );
    drop(c);
    drop(cold);
    drop(_root);
}
