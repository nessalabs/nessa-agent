//! Actual Auth facts through the stop owner; no recipient or product mount.
use super::*;
use crate::webhooks::application::{
    ObserveRegistrationStop, RegistrationStop, RegistrationStops, StopFailure,
    StopObservationResult,
};
use crate::webhooks::infrastructure::outbox::{retirement, stop::SavedStop};
use nessa_auth::application::{
    ports::{AccessAdmission, AccessError, AccessHold, AccessReader, AccessSnapshot, PortFuture},
    revocation::RevocationRecord,
};
use nessa_auth::domain::{Initiator, RevocationCause, Supersession, TransitionCause};
use std::sync::atomic::AtomicUsize;

fn audits(f: &Registered) -> Vec<Value> {
    let mut values: Vec<Value> = std::fs::read_dir(&f.audit)
        .unwrap()
        .map(|entry| {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!text.contains("nessa_v1."));
            assert!(!text.contains("private-only-fixture"));
            assert!(!text.contains("recipient.test"));
            serde_json::from_str(&text).unwrap()
        })
        .collect();
    values.sort_by_key(|v| v["recordId"].as_str().unwrap().to_owned());
    values
}

#[tokio::test]
async fn actual_explicit_fact_and_first_observation_survive_retirement_ack_loss() {
    let mut f = Registered::new();
    let proposal = f.proposed();
    let id = proposal.id;
    let credential = CredentialId::new(&proposal.issue.credential_id).unwrap();
    f.command(&f.store).execute(proposal).await.unwrap();
    let original_files = audits(&f);
    let original_bytes = std::fs::read(&f.path).unwrap();
    assert_eq!(
        ObserveRegistrationStop {
            credentials: &f.auth,
            stops: &f.store,
            clock: f.clock.as_ref()
        }
        .execute(&credential, ExpiryObserver::Restart)
        .await
        .unwrap(),
        StopObservationResult::NoStopObserved
    );
    assert_eq!(audits(&f), original_files);
    assert_eq!(std::fs::read(&f.path).unwrap(), original_bytes);
    let canonical = f
        .auth
        .revoke_sync(RevokeCredentialRequest {
            request_id: "actual-stop".into(),
            issuer_principal_id: "owner".into(),
            credential_id: credential.as_str().into(),
            revoked_at: 101,
        })
        .unwrap();
    let original = f
        .auth
        .read(&credential)
        .await
        .unwrap()
        .revocation_fact()
        .unwrap()
        .unwrap()
        .clone();
    f.clock.0.store(101_007, Ordering::SeqCst);
    f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER hold_stop_retirement BEFORE UPDATE OF published ON audit WHEN json_extract(OLD.record,'$.cause')='drained' BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    assert_eq!(
        ObserveRegistrationStop {
            credentials: &f.auth,
            stops: &f.store,
            clock: f.clock.as_ref()
        }
        .execute(&credential, ExpiryObserver::Restart)
        .await,
        Err(StopFailure::Store(OutboxError::Conflict))
    );
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 0);
    {
        let c = f.store.owner.connection.lock().unwrap();
        let retired = retirement::read(&c, id).unwrap().unwrap();
        assert_eq!(retired.stop.observation().at_ms(), 101_007);
        assert_eq!(
            SavedStop::of(&retired.stop).encode().unwrap(),
            SavedStop::of(&RegistrationStop::revoked(&original, 101_007))
                .encode()
                .unwrap()
        );
        c.execute_batch("DROP TRIGGER hold_stop_retirement")
            .unwrap();
    }
    let before = audits(&f);
    // A canonical-shaped but substituted original journal identity must be
    // rejected before the retained retirement acknowledgement is retried.
    for field in ["sequence", "revision", "correlation"] {
        let mut value = serde_json::to_value(original.retained_record().to_dto()).unwrap();
        value[field] = if field == "correlation" {
            json!("substituted-command")
        } else {
            json!(value[field].as_u64().unwrap() + 1)
        };
        let record = RevocationRecord::decode(serde_json::from_value(value).unwrap()).unwrap();
        let substituted = RegistrationStop::restore_revocation(record, 102_000);
        let held_directory = f.audit.with_extension("held");
        std::fs::rename(&f.audit, &held_directory).unwrap();
        assert_eq!(
            f.store
                .stop_credential(credential.clone(), substituted)
                .await,
            Err(OutboxError::Invalid),
            "substituted {field} reached owed publication"
        );
        assert!(!f.audit.exists());
        std::fs::rename(held_directory, &f.audit).unwrap();
        assert_eq!(audits(&f), before);
    }
    for value in before
        .iter()
        .filter(|v| v["cause"] == "credential_revoked" || v["cause"] == "drained")
    {
        assert_eq!(
            value["credentialStop"]["revocation"],
            serde_json::to_value(&canonical.revocation).unwrap()
        );
        assert_eq!(value["credentialStop"]["atMs"], 101_007);
        assert_eq!(value["credentialStop"]["revocation"]["at"], 101);
        assert_eq!(
            value["credentialStop"]["revocation"]["after"]["revokedAt"],
            101
        );
        assert_eq!(value["initiator"], "principal:owner");
    }
    // A shape-valid stored journal substitution must also fail contextual
    // restoration/publication, even without a new supplied stop operation.
    let original_stop = {
        let c = f.store.owner.connection.lock().unwrap();
        c.query_row(
            "SELECT stop FROM retirements WHERE id=?1",
            [id.to_string()],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };
    let mut altered: Value = serde_json::from_str(&original_stop).unwrap();
    altered["revocation"]["sequence"] = json!(original.sequence() + 1);
    {
        let c = f.store.owner.connection.lock().unwrap();
        c.execute(
            "UPDATE retirements SET stop=?1 WHERE id=?2",
            params![altered.to_string(), id.to_string()],
        )
        .unwrap();
        // This is locally valid, and differs only in its original journal ID.
        retirement::read(&c, id).unwrap().unwrap();
    }
    let held_directory = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held_directory).unwrap();
    assert_eq!(f.store.due(102_000).await.err(), Some(OutboxError::Invalid));
    assert!(!f.audit.exists());
    std::fs::rename(held_directory, &f.audit).unwrap();
    f.store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute(
            "UPDATE retirements SET stop=?1 WHERE id=?2",
            params![original_stop, id.to_string()],
        )
        .unwrap();
    f.reopen();
    f.store
        .stop_credential(
            credential.clone(),
            RegistrationStop::revoked(&original, 102_000),
        )
        .await
        .unwrap();
    assert_eq!(audits(&f), before);
    assert_eq!(
        f.auth
            .read(&credential)
            .await
            .unwrap()
            .revocation_fact()
            .unwrap(),
        Some(&original)
    );
}

#[tokio::test]
async fn actual_supersession_records_preserve_offline_actor_and_distinct_auth_times() {
    for recovery in [false, true] {
        let f = Registered::new();
        // These are actual Auth events tested only as retained codec inputs.
        // No webhook registration is installed for a non-webhook origin.
        let credential = if recovery {
            let _new = f
                .auth
                .recover_owner("replacement-owner".into(), &["credential.manage"], 30, None)
                .unwrap();
            CredentialId::new("owner-key").unwrap()
        } else {
            let _first = f
                .auth
                .provision_surface(
                    "panel",
                    "first".into(),
                    "surface-one".into(),
                    vec!["server.read".into()],
                    20,
                    None,
                )
                .unwrap();
            let _second = f
                .auth
                .provision_surface(
                    "panel",
                    "second".into(),
                    "surface-two".into(),
                    vec!["server.read".into()],
                    19,
                    None,
                )
                .unwrap();
            CredentialId::new("surface-one").unwrap()
        };
        let held = f.auth.hold(&credential).await.unwrap();
        let fact = held.snapshot.revocation_fact().unwrap().unwrap();
        let stop = RegistrationStop::revoked(fact, 40_123);
        let encoded = SavedStop::of(&stop).encode().unwrap();
        assert_eq!(SavedStop::decode(&encoded).unwrap(), stop);
        let mut substituted: Value = serde_json::from_str(&encoded).unwrap();
        substituted["initiator"] = json!("principal:substituted-reader");
        assert_eq!(
            SavedStop::decode(&substituted.to_string()),
            Err(OutboxError::Invalid)
        );
        assert_eq!(stop.observation().at_ms(), 40_123);
        assert!(matches!(
            fact.transition().cause(),
            TransitionCause::Revoked(RevocationCause::Superseded { .. })
        ));
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            value["revocation"],
            serde_json::to_value(fact.retained_record().to_dto()).unwrap()
        );
        if recovery {
            assert_eq!(fact.transition().initiator(), &Initiator::LocalOperator);
            assert_eq!(value["initiator"], "local_operator");
            assert!(matches!(
                fact.transition().cause(),
                TransitionCause::Revoked(RevocationCause::Superseded {
                    kind: Supersession::OwnerRecovery,
                    ..
                })
            ));
        } else {
            assert_eq!(
                fact.transition().initiator(),
                &Initiator::Principal(PrincipalId::new("owner").unwrap())
            );
            assert_eq!(fact.transition().at(), 19);
            assert_eq!(fact.transition().after().revoked_at, Some(20));
            assert_eq!(value["revocation"]["at"], 19);
            assert_eq!(value["revocation"]["after"]["revokedAt"], 20);
        }
    }
}

#[tokio::test]
async fn different_actual_credential_cannot_publish_a_pending_registration_obligation() {
    let f = Registered::new();
    let proposal = f.proposed();
    let credential = CredentialId::new(&proposal.issue.credential_id).unwrap();
    f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER lose_initial_stop_ack BEFORE UPDATE OF published ON audit BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    assert!(matches!(
        f.command(&f.store).execute(proposal).await,
        Err(RegistrationFailure::Store(OutboxError::Conflict))
    ));
    f.store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER lose_initial_stop_ack")
        .unwrap();
    let _replacement = f
        .auth
        .recover_owner(
            "different-owner-key".into(),
            &["credential.manage"],
            30,
            None,
        )
        .unwrap();
    let other_id = CredentialId::new("owner-key").unwrap();
    let held = f.auth.hold(&other_id).await.unwrap();
    let stop = RegistrationStop::revoked(held.snapshot.revocation_fact().unwrap().unwrap(), 30_001);
    let held_directory = f.audit.with_extension("held");
    std::fs::rename(&f.audit, &held_directory).unwrap();
    assert_eq!(
        f.store.stop_credential(credential.clone(), stop).await,
        Err(OutboxError::Invalid)
    );
    assert!(!f.audit.exists());
    {
        let c = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM audit WHERE published=0", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    std::fs::rename(held_directory, &f.audit).unwrap();
    f.store.due(30_002).await.unwrap();
    assert_eq!(f.count("registrations"), 1);
}

struct SuppliedSnapshot(AccessSnapshot);
impl AccessAdmission for SuppliedSnapshot {
    fn hold<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessHold> {
        Box::pin(async move { Ok(AccessHold::held(self.0.clone(), ())) })
    }
}
struct CountStops(AtomicUsize);
#[async_trait]
impl RegistrationStops for CountStops {
    async fn stop_credential(
        &self,
        _: CredentialId,
        _: RegistrationStop,
    ) -> Result<(), OutboxError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn missing_actual_revocation_never_falls_back_to_expiry_or_caller_actor() {
    let f = Registered::new();
    let proposal = f.proposed();
    let credential = CredentialId::new(&proposal.issue.credential_id).unwrap();
    f.command(&f.store).execute(proposal).await.unwrap();
    f.auth
        .revoke_sync(RevokeCredentialRequest {
            request_id: "missing-stop-record".into(),
            issuer_principal_id: "owner".into(),
            credential_id: credential.as_str().into(),
            revoked_at: 101,
        })
        .unwrap();
    let mut snapshot = f.auth.read(&credential).await.unwrap();
    snapshot.revocation = None; // Explicit malformed read-provider response.
    f.clock.0.store(102_000, Ordering::SeqCst);
    let stops = CountStops(AtomicUsize::new(0));
    assert_eq!(
        ObserveRegistrationStop {
            credentials: &SuppliedSnapshot(snapshot),
            stops: &stops,
            clock: f.clock.as_ref()
        }
        .execute(
            &credential,
            ExpiryObserver::Owner(PrincipalId::new("unrelated-reader").unwrap())
        )
        .await,
        Err(StopFailure::Access(AccessError::Unavailable))
    );
    let other = f
        .auth
        .read(&CredentialId::new("owner-key").unwrap())
        .await
        .unwrap();
    assert_eq!(
        ObserveRegistrationStop {
            credentials: &SuppliedSnapshot(other),
            stops: &stops,
            clock: f.clock.as_ref()
        }
        .execute(&credential, ExpiryObserver::Restart)
        .await,
        Err(StopFailure::Access(AccessError::IdentityMismatch))
    );
    assert_eq!(stops.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn not_yet_valid_is_not_an_expiry_observation() {
    let f = Registered::new();
    let mut request = f.proposed().issue;
    request.request_id = "future-credential".into();
    request.issued_at = 20;
    let credential = CredentialId::new(&request.credential_id).unwrap();
    let _issued = f.auth.issue_sync(request).unwrap();
    let snapshot = f.auth.read(&credential).await.unwrap();
    assert!(!snapshot.credential.is_valid_at(10));
    assert_eq!(snapshot.credential.revoked_at(), None);
    let stops = CountStops(AtomicUsize::new(0));
    assert_eq!(
        ObserveRegistrationStop {
            credentials: &f.auth,
            stops: &stops,
            clock: f.clock.as_ref()
        }
        .execute(&credential, ExpiryObserver::Restart)
        .await
        .unwrap(),
        StopObservationResult::NoStopObserved
    );
    assert_eq!(stops.0.load(Ordering::SeqCst), 0);
}
