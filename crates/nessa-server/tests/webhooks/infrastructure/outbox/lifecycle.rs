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
                    observed_by: ExpiryObserver::Restart,
                },
                100_000
            )
            .await,
        Err(OutboxError::Unavailable)
    );
    assert_eq!(f.count("registrations"), 1);
    assert_eq!(f.count("registration_intents"), 1);
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
    assert_eq!(saved.len(), 1);
    for (id, text) in &saved {
        let mut value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(value["initiator"], "restart");
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
    assert_eq!(f.count("registrations"), 1);
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
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 0);
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
        assert_eq!(value["initiator"], "restart");
        assert_eq!(value["atMs"], 100_000);
    }
    assert!(logs
        .iter()
        .any(|v| v["cause"] == "drained" && v["initiator"] == "restart"));
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
    fn decode(
        actor: &str,
        at_ms: u64,
    ) -> Result<crate::webhooks::application::RegistrationStop, OutboxError> {
        let text = json!({"cause":"credential_expired","initiator":actor,"atMs":at_ms}).to_string();
        crate::webhooks::infrastructure::outbox::stop::SavedStop::decode(&text)
    }
    let delivery = Uuid::new_v4();
    for (actor, expected) in [
        ("restart".to_owned(), ExpiryObserver::Restart),
        (
            "owner:owner".to_owned(),
            ExpiryObserver::Owner(nessa_auth::domain::PrincipalId::new("owner").unwrap()),
        ),
        (
            format!("delivery:{delivery}"),
            ExpiryObserver::Delivery(delivery),
        ),
    ] {
        let stop = decode(&actor, 12_000).unwrap();
        assert_eq!(
            stop.observation().reason(),
            &StopReason::Expired {
                observed_by: expected
            }
        );
        assert_eq!(registry::stop_actor(stop.observation()), actor);
        assert!(matches!(
            stop.observation().reason(),
            StopReason::Expired { .. }
        ));
    }
    for actor in [
        "lifecycle".to_owned(),
        "Lifecycle".to_owned(),
        "owner:".into(),
        "lifecycle ".into(),
        format!("lifecycle:{delivery}"),
        format!("reader:{delivery}"),
        "delivery:bad".into(),
        format!("delivery:{}", delivery.to_string().to_uppercase()),
    ] {
        assert!(
            matches!(decode(&actor, 12_000), Err(OutboxError::Invalid)),
            "{actor}"
        );
    }
    assert!(matches!(
        decode("restart", u64::MAX),
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
                    observed_by: ExpiryObserver::Restart,
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
            assert_eq!(value["initiator"], "restart");
            if value["after"] == "stopped" {
                assert_eq!(value["atMs"], 2001);
            }
        }
        assert!(records.iter().any(|v| v["after"] == "absent"
            && v["initiator"] == "restart"
            && v["stopAtMs"] == 2001));
    }
}

#[tokio::test]
async fn owner_expiry_observation_survives_failed_stop_and_later_canonical_revocation() {
    let mut f = Registered::new();
    let proposed = f.proposed();
    let credential = CredentialId::new(&proposed.issue.credential_id).unwrap();
    let _issued = f.command(&f.store).execute(proposed).await.unwrap();
    f.clock.0.store(100_000, Ordering::SeqCst);
    // Fail only the second transaction. The first observation must already be
    // durable and acknowledged, with its actual owner and original read time.
    f.store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fixture_stop_failure BEFORE UPDATE OF active ON registrations
         WHEN NEW.active=0 BEGIN SELECT RAISE(ABORT,'fixture stop failure'); END;",
        )
        .unwrap();
    assert_eq!(
        f.store
            .stop(
                credential.clone(),
                StopReason::Expired {
                    observed_by: ExpiryObserver::Owner(PrincipalId::new("owner").unwrap()),
                },
                100_000
            )
            .await,
        Err(OutboxError::Unavailable),
    );
    {
        let c = f.store.owner.connection.lock().unwrap();
        let saved: (i64, String, String, i64) = c
            .query_row(
                "SELECT active,json_extract(stop,'$.cause'),json_extract(stop,'$.initiator'),json_extract(stop,'$.atMs') FROM registrations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            saved,
            (
                1,
                "credential_expired".into(),
                "owner:owner".into(),
                100_000
            )
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM audit WHERE published=0", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        c.execute_batch("DROP TRIGGER fixture_stop_failure")
            .unwrap();
    }
    f.reopen();
    let canonical = f
        .auth
        .revoke_sync(RevokeCredentialRequest {
            request_id: "revoke-after-expiry-observation".into(),
            issuer_principal_id: "owner".into(),
            credential_id: credential.as_str().into(),
            revoked_at: 101,
        })
        .unwrap();
    assert!(matches!(
        canonical.revocation.initiator,
        InitiatorDto::Principal { .. }
    ));
    assert_eq!(canonical.revocation.at, 101);
    use nessa_auth::application::ports::AccessAdmission;
    let held = f.auth.hold(&credential).await.unwrap();
    let original = held.snapshot.revocation_fact().unwrap().unwrap();
    let stop = crate::webhooks::application::RegistrationStop::revoked(original, 101_000);
    f.store.stop_credential(credential, stop).await.unwrap();
    drop(held);
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 0);
    let logs = std::fs::read_dir(&f.audit)
        .unwrap()
        .map(|entry| {
            serde_json::from_str::<Value>(&std::fs::read_to_string(entry.unwrap().path()).unwrap())
                .unwrap()
        })
        .collect::<Vec<_>>();
    for cause in ["credential_expiry_observed", "credential_expired"] {
        let records = logs
            .iter()
            .filter(|v| v["cause"] == cause)
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 1, "{cause}");
        assert_eq!(records[0]["initiator"], "owner:owner");
        assert_eq!(records[0]["atMs"], 100_000);
    }
    let drained = logs.iter().find(|v| v["cause"] == "drained").unwrap();
    assert_eq!(drained["initiator"], "owner:owner");
    assert_eq!(drained["stopAtMs"], 100_000);
    f.reopen();
    assert!(f.store.due(102_000).await.unwrap().is_empty());
}

#[tokio::test]
async fn owed_observation_and_retired_stop_refuse_cross_record_substitutions_before_publication() {
    for stopped in [false, true] {
        let mut f = Registered::new();
        let proposed = f.proposed();
        let credential = CredentialId::new(&proposed.issue.credential_id).unwrap();
        let _issued = f.command(&f.store).execute(proposed).await.unwrap();
        f.clock.0.store(100_000, Ordering::SeqCst);
        if stopped {
            f.store
                .observe_expiry(credential.clone(), ExpiryObserver::Restart, 100_000)
                .await
                .unwrap();
        }
        let held = f.audit.with_extension("held");
        std::fs::rename(&f.audit, &held).unwrap();
        let outcome = if stopped {
            f.store
                .stop(
                    credential.clone(),
                    StopReason::Expired {
                        observed_by: ExpiryObserver::Restart,
                    },
                    100_000,
                )
                .await
        } else {
            f.store
                .observe_expiry(credential.clone(), ExpiryObserver::Restart, 100_000)
                .await
        };
        assert_eq!(outcome, Err(OutboxError::Unavailable));
        let rows = {
            let c = f.store.owner.connection.lock().unwrap();
            let mut q = c
                .prepare("SELECT id,record FROM audit WHERE published=0")
                .unwrap();
            q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(!rows.is_empty());
        for (id, original) in rows {
            let value: Value = serde_json::from_str(&original).unwrap();
            let mut changes = vec![
                ("initiator", json!("owner:other-reader")),
                ("atMs", json!(100_001)),
                ("credential", json!("other-credential")),
                ("principal", json!("other-principal")),
                ("organization", json!("other-organization")),
                ("owner", json!("other-owner")),
                ("registration", json!(Uuid::new_v4().to_string())),
            ];
            if value["cause"] == "credential_expiry_observed" {
                changes.push(("after", json!("stopped")));
            } else if value["cause"] == "credential_expired" {
                changes.push(("before", json!("active")));
            } else {
                assert_eq!(value["cause"], "drained");
                changes.push(("stopAtMs", json!(100_001)));
            }
            for (field, replacement) in changes {
                let mut altered = value.clone();
                assert!(altered.get(field).is_some(), "missing {field}: {value}");
                assert_ne!(altered[field], replacement);
                altered[field] = replacement;
                f.store
                    .owner
                    .connection
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE audit SET record=?1 WHERE id=?2",
                        params![serde_json::to_string(&altered).unwrap(), id],
                    )
                    .unwrap();
                assert!(
                    matches!(f.store.due(100_000).await, Err(OutboxError::Invalid)),
                    "{stopped}/{field}: {altered}"
                );
                // With the sink unavailable, contextual refusal must still win
                // before attempting publication or acknowledging any audit row.
                assert!(!f.audit.exists());
                f.store
                    .owner
                    .connection
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE audit SET record=?1 WHERE id=?2",
                        params![original, id],
                    )
                    .unwrap();
            }
        }
        std::fs::rename(held, &f.audit).unwrap();
        f.reopen();
        assert!(f.store.due(100_000).await.unwrap().is_empty());
        if !stopped {
            f.store
                .stop(
                    credential,
                    StopReason::Expired {
                        observed_by: ExpiryObserver::Restart,
                    },
                    101_000,
                )
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn first_expiry_and_actual_auth_revocation_remain_distinct_after_retirement() {
    use nessa_auth::application::ports::{AccessAdmission, AccessReader};
    use nessa_auth::domain::{Initiator, RevocationCause, TransitionCause};
    for first in [
        ExpiryObserver::Owner(PrincipalId::new("owner").unwrap()),
        ExpiryObserver::Restart,
    ] {
        let mut f = Registered::new();
        let proposed = f.proposed();
        let id = CredentialId::new(&proposed.issue.credential_id).unwrap();
        f.command(&f.store).execute(proposed).await.unwrap();
        f.clock.0.store(100_000, Ordering::SeqCst);
        let expired = f.auth.read(&id).await.unwrap();
        assert!(!expired.credential.is_valid_at(100));
        assert_eq!(expired.revocation_fact().unwrap(), None);
        let expected = match &first {
            ExpiryObserver::Owner(p) => format!("owner:{}", p.as_str()),
            ExpiryObserver::Restart => "restart".into(),
            ExpiryObserver::Delivery(_) => unreachable!(),
        };
        f.store
            .observe_expiry(id.clone(), first, 100_001)
            .await
            .unwrap();
        f.reopen();
        let committed = f
            .auth
            .revoke_sync(RevokeCredentialRequest {
                request_id: "actual-after-expiry".into(),
                issuer_principal_id: "owner".into(),
                credential_id: id.as_str().into(),
                revoked_at: 101,
            })
            .unwrap();
        let held = f.auth.hold(&id).await.unwrap();
        let fact = held.snapshot.revocation_fact().unwrap().unwrap().clone();
        assert_eq!(held.snapshot.origin, expired.origin);
        assert_eq!(fact.sequence(), committed.revocation.sequence);
        assert_eq!(fact.revision(), committed.revocation.revision);
        assert_eq!(fact.correlation(), Some("actual-after-expiry"));
        assert_eq!(fact.transition().at(), 101);
        assert_eq!(
            fact.transition().cause(),
            &TransitionCause::Revoked(RevocationCause::Explicit)
        );
        let Initiator::Principal(actor) = fact.transition().initiator() else {
            panic!("actual explicit owner command has its original principal")
        };
        // This exercises the two existing owners together. The general runtime
        // coordinator and offline/supersession mapping remain separate work.
        f.store
            .stop(
                id.clone(),
                StopReason::Revoked {
                    initiator: nessa_auth::domain::Initiator::Principal(actor.clone()),
                    cause: nessa_auth::domain::RevocationCause::Explicit,
                },
                101_000,
            )
            .await
            .unwrap();
        drop(held);
        assert_eq!(f.count("registrations"), 0);
        assert_eq!(f.count("registration_intents"), 0);
        f.reopen();
        let records = std::fs::read_dir(&f.audit)
            .unwrap()
            .map(|e| {
                serde_json::from_slice::<Value>(&std::fs::read(e.unwrap().path()).unwrap()).unwrap()
            })
            .collect::<Vec<_>>();
        for cause in [
            "credential_expiry_observed",
            "credential_expired",
            "drained",
        ] {
            let record = records.iter().find(|v| v["cause"] == cause).unwrap();
            assert_eq!(record["initiator"], expected);
            if cause == "drained" {
                assert_eq!(record["stopAtMs"], 100_001);
            } else {
                assert_eq!(record["atMs"], 100_001);
            }
        }
        let private = f._root.path().join("private");
        drop(f.auth);
        let auth = LocalCredentialStore::open(
            &private,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap();
        assert_eq!(
            auth.read(&id).await.unwrap().revocation_fact().unwrap(),
            Some(&fact)
        );
    }
}
