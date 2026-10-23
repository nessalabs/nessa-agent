//! Typed automatic expiry observations and their original retained cause.
use super::super::Fixture;
use super::*;

#[tokio::test]
async fn lifecycle_expiry_before_a_delivery_retains_original_observer_through_lost_ack_and_restore()
{
    let mut f = Registered::new();
    let proposed = f.proposed();
    let credential = CredentialId::new(&proposed.issue.credential_id).unwrap();
    let _issued = f.command(&f.store).execute(proposed).await.unwrap();
    f.clock.0.store(100_000, Ordering::SeqCst);
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held).unwrap();
    assert_eq!(
        f.store
            .stop(
                credential.clone(),
                StopReason::Expired {
                    observed_by: ExpiryObserver::Lifecycle,
                },
                100_000
            )
            .await,
        Err(OutboxError::Unavailable)
    );
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 0);
    let saved = {
        let c = f.store.owner.connection.lock().unwrap();
        let mut q = c
            .prepare("SELECT id,record FROM audit WHERE published=0")
            .unwrap();
        q.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    assert_eq!(saved.len(), 2);
    for (id, text) in &saved {
        let mut value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(value["initiator"], "lifecycle");
        value["initiator"] = json!(format!("delivery:{}", Uuid::new_v4()));
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE audit SET record=?1 WHERE id=?2",
                params![serde_json::to_string(&value).unwrap(), id],
            )
            .unwrap();
        assert!(matches!(
            f.store.due(100_000).await,
            Err(OutboxError::Invalid)
        ));
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .execute("UPDATE audit SET record=?1 WHERE id=?2", params![text, id])
            .unwrap();
    }
    std::fs::rename(held, &f.audit).unwrap();
    f.reopen();
    assert!(f.store.due(100_000).await.unwrap().is_empty());
    // A later observer cannot replace the already committed lifecycle fact,
    // even after the private registration has retired.
    f.store
        .stop(
            credential,
            StopReason::Expired {
                observed_by: ExpiryObserver::Delivery(Uuid::new_v4()),
            },
            101_000,
        )
        .await
        .unwrap();
    let logs = std::fs::read_dir(&f.audit)
        .unwrap()
        .map(|p| {
            serde_json::from_str::<Value>(&std::fs::read_to_string(p.unwrap().path()).unwrap())
                .unwrap()
        })
        .collect::<Vec<_>>();
    let stops = logs
        .iter()
        .filter(|v| v["cause"] == "credential_expired")
        .collect::<Vec<_>>();
    assert!(!stops.is_empty());
    for value in stops {
        assert_eq!(value["initiator"], "lifecycle");
        assert_eq!(value["atMs"], 100_000);
    }
    assert!(logs
        .iter()
        .any(|v| v["cause"] == "drained" && v["initiator"] == "lifecycle"));
}

#[tokio::test]
async fn delivery_expiry_observation_keeps_the_actual_queued_identity() {
    let f = Fixture::new().await;
    let queued = f.record(1);
    let id = queued.id;
    f.store.queue(queued).await.unwrap();
    f.store
        .stop(
            f.registration.target.credential.clone(),
            StopReason::Expired {
                observed_by: ExpiryObserver::Delivery(id),
            },
            12_000,
        )
        .await
        .unwrap();
    let records = f.audits();
    assert!(records
        .iter()
        .any(|v| v["cause"] == "credential_expired" && v["initiator"] == format!("delivery:{id}")));
    assert!(records
        .iter()
        .filter(|v| v["cause"] == "credential_expired")
        .all(|v| v["initiator"] == format!("delivery:{id}")));
    assert!(f.store.due(12_000).await.unwrap().is_empty());
}

#[test]
fn automatic_expiry_observer_decoding_is_closed_and_canonical() {
    let delivery = Uuid::new_v4();
    for (actor, expected) in [
        ("lifecycle".to_owned(), ExpiryObserver::Lifecycle),
        (
            format!("delivery:{delivery}"),
            ExpiryObserver::Delivery(delivery),
        ),
    ] {
        let stop = registry::Stopped::restore("credential_expired", &actor, 12_000).unwrap();
        assert_eq!(
            stop.reason(),
            &StopReason::Expired {
                observed_by: expected
            }
        );
        assert_eq!(stop.initiator(), actor);
        assert_eq!(stop.cause(), "credential_expired");
    }
    for actor in [
        "Lifecycle".to_owned(),
        "lifecycle ".into(),
        format!("lifecycle:{delivery}"),
        format!("reader:{delivery}"),
        "delivery:bad".into(),
        format!("delivery:{}", delivery.to_string().to_uppercase()),
    ] {
        assert!(
            matches!(
                registry::Stopped::restore("credential_expired", &actor, 12_000),
                Err(OutboxError::Invalid)
            ),
            "{actor}"
        );
    }
    assert!(matches!(
        registry::Stopped::restore("credential_expired", "lifecycle", u64::MAX),
        Err(OutboxError::Invalid)
    ));
}

#[tokio::test]
async fn first_lifecycle_stop_survives_later_observer_and_sending_recovery() {
    for crash in [false, true] {
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
                    observed_by: ExpiryObserver::Lifecycle,
                },
                2001,
            )
            .await
            .unwrap();
        f.store
            .stop(
                f.registration.target.credential.clone(),
                StopReason::Expired {
                    observed_by: ExpiryObserver::Delivery(sending.id()),
                },
                2002,
            )
            .await
            .unwrap();
        if crash {
            let replacement = Fixture::new().await;
            drop(std::mem::replace(&mut f.store, replacement.store));
            f.store = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
            f.store.recover(3000).await.unwrap();
        } else {
            let retry = sending.observed(Observation::Retryable, 2100).unwrap();
            f.store
                .transition(&sending, &retry, DeliveryCause::Response, 2100)
                .await
                .unwrap();
        }
        assert!(f.store.due(5000).await.unwrap().is_empty());
        let records = f.audits();
        assert!(records.iter().any(|v| v["cause"] == "credential_expired"));
        for value in records
            .iter()
            .filter(|v| v["cause"] == "credential_expired")
        {
            assert_eq!(value["initiator"], "lifecycle");
            if value["after"] == "stopped" {
                assert_eq!(value["atMs"], 2001);
            }
        }
        assert!(records.iter().any(|v| v["after"] == "absent"
            && v["initiator"] == "lifecycle"
            && v["stopAtMs"] == 2001));
    }
}
