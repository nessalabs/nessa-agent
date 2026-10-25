//! Positive retained response evidence, before owed publication and reclamation.
use super::*;

#[tokio::test]
async fn response_reconciliation_requires_original_transition_and_time_before_effects() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(10_000).await.unwrap().pop().unwrap().delivery;
    let sending = queued.admit(7, 10_000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 10_000)
        .await
        .unwrap();
    let observed = sending.observed(Observation::Success, 10_001).unwrap();
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store.settle_response(&sending, &observed, 10_001).await,
        Err(OutboxError::Unavailable)
    );
    let files = std::fs::read_dir(&held).unwrap().count();
    let rejected = sending.observed(Observation::Rejected, 10_001).unwrap();
    assert_eq!(
        f.store.settle_response(&sending, &rejected, 10_001).await,
        Err(OutboxError::Conflict)
    );
    assert_eq!(
        f.store.settle_response(&sending, &observed, 10_002).await,
        Err(OutboxError::Conflict)
    );
    assert_eq!(std::fs::read_dir(&held).unwrap().count(), files);
    assert!(
        !f.audit.exists(),
        "contradictory input cannot publish original owed evidence"
    );
    std::fs::rename(&held, &f.audit).unwrap();
    f.store
        .settle_response(&sending, &observed, 10_001)
        .await
        .unwrap();
    f.store
        .settle_response(&sending, &observed, 10_001)
        .await
        .unwrap();
    assert!(f.store.due(10_002).await.unwrap().is_empty());
    assert_eq!(
        f.audits()
            .iter()
            .filter(|value| value["cause"] == "response")
            .count(),
        1
    );
    // The original CAS port keeps its existing conflict semantics.
    assert_eq!(
        f.store
            .transition(&sending, &observed, DeliveryCause::Response, 10_001)
            .await,
        Err(OutboxError::Conflict)
    );
}

#[tokio::test]
async fn erased_delivery_or_response_evidence_never_establishes_acknowledgement() {
    for missing in ["delivery", "response", "published_owner"] {
        let f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        let queued = f.store.due(10_000).await.unwrap().pop().unwrap().delivery;
        let sending = queued.admit(7, 10_000).unwrap();
        f.store
            .transition(&queued, &sending, DeliveryCause::Admission, 10_000)
            .await
            .unwrap();
        let observed = sending.observed(Observation::Success, 10_001).unwrap();
        let held = f.audit.with_extension("held");
        std::fs::rename(&f.audit, &held).unwrap();
        assert_eq!(
            f.store.settle_response(&sending, &observed, 10_001).await,
            Err(OutboxError::Unavailable)
        );
        if missing == "published_owner" {
            std::fs::rename(&held, &f.audit).unwrap();
            f.store
                .settle_response(&sending, &observed, 10_001)
                .await
                .unwrap();
            std::fs::rename(&f.audit, &held).unwrap();
        }
        let connection = Connection::open(&f.path).unwrap();
        if missing == "delivery" {
            connection.execute("DELETE FROM deliveries", []).unwrap();
        } else if missing == "response" {
            connection
                .execute(
                    "DELETE FROM audit WHERE json_extract(record,'$.cause')='response'",
                    [],
                )
                .unwrap();
        }
        if missing == "published_owner" {
            connection.execute("UPDATE audit SET record=json_set(record,'$.owner','another-owner') WHERE json_extract(record,'$.cause')='response'", []).unwrap();
        }
        drop(connection);
        let files = std::fs::read_dir(&held).unwrap().count();
        assert!(
            f.store
                .settle_response(&sending, &observed, 10_001)
                .await
                .is_err(),
            "{missing}"
        );
        assert_eq!(std::fs::read_dir(&held).unwrap().count(), files);
        assert!(!f.audit.exists());
    }
}

#[tokio::test]
async fn acknowledged_sending_can_end_without_dispatch_and_preserve_its_actual_audit() {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(10_000).await.unwrap().pop().unwrap().delivery;
    let sending = queued.admit(7, 10_000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 10_000)
        .await
        .unwrap();
    let refused = sending.not_dispatched().unwrap();
    assert_eq!(
        f.store
            .transition(&sending, &refused, DeliveryCause::AdmissionExpired, 10_001)
            .await,
        Ok(())
    );
    assert!(f.store.due(10_001).await.unwrap().is_empty());
    assert_eq!(
        f.audits()
            .iter()
            .filter(|fact| fact["cause"] == "admission_expired")
            .count(),
        1
    );
}

#[tokio::test]
async fn late_response_acknowledgement_keeps_the_actual_full_retention_boundary() {
    let mut f = Fixture::new().await;
    f.conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let other = ConversationId::new("ffffffff-ffff-4fff-bfff-ffffffffffff").unwrap();
    let mut delayed = f.record(1);
    delayed.conversation = other.clone();
    delayed.scope = Scope::new(
        Id::new(f.registration.id.to_string()).unwrap(),
        Id::new("origin").unwrap(),
        Id::new(other.to_string()).unwrap(),
        Id::new("incarnation-a").unwrap(),
        Id::new("records-v1").unwrap(),
        Id::new("epoch-7").unwrap(),
    );
    let original = f.store.queue(delayed).await.unwrap();
    super::retention_fixture::acknowledged_prehistory(&f, RETAINED_LIMIT, 0, true).await;
    let queued = f.store.due(4000).await.unwrap().pop().unwrap().delivery;
    assert_eq!(queued.id(), original);
    let sending = queued.admit(7, 4000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 4000)
        .await
        .unwrap();
    let observed = sending.observed(Observation::Success, 4001).unwrap();
    f.store
        .settle_response(&sending, &observed, 4001)
        .await
        .unwrap();
    let connection = Connection::open(&f.path).unwrap();
    let retained: i64 = connection
        .query_row(
            "SELECT count(*) FROM deliveries WHERE phase NOT IN ('queued','sending','retry')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        retained, RETAINED_LIMIT as i64,
        "actual acknowledgement must also finish bounded reclamation"
    );
    drop(connection);
    assert!(f.store.due(4001).await.unwrap().is_empty());
}
