//! Actual issued-but-uninstalled cleanup; no recipient or product mounting.
use super::*;
use crate::webhooks::application::{
    ObserveRegistrationStop, RegistrationStop, StopFailure, UninstalledCancellation,
};
use crate::webhooks::infrastructure::outbox::{audit, retirement, stop::SavedStop};
use nessa_auth::application::{
    credential_admin::CredentialAdmin,
    ports::{AccessAdmission, AccessHold, AccessReader, AccessSnapshot, PortFuture},
};
use std::collections::BTreeMap;
use std::io::Write;
use tokio::sync::Notify;

fn files(f: &Registered) -> BTreeMap<String, String> {
    std::fs::read_dir(&f.audit)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(!text.contains("private-only-fixture"));
            assert!(!text.contains("recipient.test"));
            assert!(!text.contains("nessa_v1."));
            (path.file_name().unwrap().to_str().unwrap().to_owned(), text)
        })
        .collect()
}

fn actual_stop(snapshot: &AccessSnapshot, at: u64) -> RegistrationStop {
    match snapshot.revocation_fact().unwrap() {
        Some(fact) => RegistrationStop::revoked(fact, at),
        None => {
            assert!(snapshot
                .credential
                .expires_at()
                .is_some_and(|expiry| at / 1000 >= expiry));
            RegistrationStop::expired(
                ExpiryObserver::Owner(PrincipalId::new("owner").unwrap()),
                at,
            )
        }
    }
}

async fn stopped_reservation(f: &Registered, mode: &str) -> (RegistrationIntent, RegistrationStop) {
    let mut proposed = f.proposed();
    if mode == "superseded" {
        // Actual offline surface replacement may supersede an existing webhook
        // integration credential. Ordinary issue does not replace credentials.
        proposed.issue.principal.id = "surface:cancel-hook".into();
        proposed.issue.membership.id = "surface:cancel-hook".into();
        proposed.issue.membership.principal_id = "surface:cancel-hook".into();
    }
    let intent = f.store.reserve(proposed).await.unwrap();
    let _issued = f.auth.issue_sync(intent.issue.clone()).unwrap();
    let at = match mode {
        "expiry" => 100_007,
        "revoked" => {
            f.auth
                .revoke_sync(RevokeCredentialRequest {
                    request_id: "cancel-before-install".into(),
                    issuer_principal_id: "owner".into(),
                    credential_id: intent.issue.credential_id.clone(),
                    revoked_at: 20,
                })
                .unwrap();
            20_007
        }
        "superseded" => {
            f.auth
                .provision_surface(
                    "cancel-hook",
                    "replacement-before-install".into(),
                    "replacement-key".into(),
                    vec!["conversation.read".into()],
                    20,
                    None,
                )
                .unwrap();
            20_007
        }
        _ => unreachable!(),
    };
    f.clock.0.store(at, Ordering::SeqCst);
    let snapshot = f
        .auth
        .read(&CredentialId::new(&intent.issue.credential_id).unwrap())
        .await
        .unwrap();
    (intent, actual_stop(&snapshot, at))
}

#[tokio::test]
async fn actual_expiry_revocation_and_supersession_cancel_without_installing_or_reissuing() {
    for mode in ["expiry", "revoked", "superseded"] {
        let f = Registered::new();
        let (intent, stop) = stopped_reservation(&f, mode).await;
        let before = f
            .auth
            .list_sync(&ListCredentialsRequest {
                organization_id: "org".into(),
            })
            .unwrap();
        assert_eq!(
            f.command(&f.store).execute(intent.clone()).await,
            Err(RegistrationFailure::Stopped),
            "{mode}"
        );
        assert_eq!(f.count("registrations"), 0);
        assert_eq!(f.count("registration_intents"), 0);
        assert_eq!(
            f.auth
                .list_sync(&ListCredentialsRequest {
                    organization_id: "org".into()
                })
                .unwrap(),
            before
        );
        let values = files(&f);
        assert_eq!(values.len(), 1);
        let record: Value = serde_json::from_str(values.values().next().unwrap()).unwrap();
        assert_eq!(record["cause"], "registration_cancelled");
        assert_eq!(record["before"], "reserved");
        assert_eq!(record["after"], "absent");
        assert_eq!(record["registration"], intent.id.to_string());
        assert_eq!(
            record["credentialStop"],
            serde_json::to_value(SavedStop::of(&stop)).unwrap()
        );
    }
}

#[tokio::test]
async fn cancellation_is_not_derived_from_a_future_issuance_or_before_expiry() {
    for at in [9_999, 99_999, 100_000] {
        let f = Registered::new();
        let intent = f.store.reserve(f.proposed()).await.unwrap();
        f.auth.issue_sync(intent.issue.clone()).unwrap();
        f.clock.0.store(at, Ordering::SeqCst);
        let outcome = f.command(&f.store).execute(intent).await;
        match at {
            9_999 => {
                assert_eq!(outcome, Err(RegistrationFailure::Stopped));
                assert_eq!(f.count("registration_intents"), 1);
                assert!(files(&f).is_empty());
                f.clock.0.store(10_000, Ordering::SeqCst);
                f.command(&f.store).execute(f.proposed()).await.unwrap();
                assert_eq!(f.count("registrations"), 1);
            }
            99_999 => {
                assert!(outcome.is_ok());
                assert_eq!(f.count("registrations"), 1);
                assert!(!files(&f)
                    .values()
                    .any(|text| text.contains("registration_cancelled")));
            }
            100_000 => {
                assert_eq!(outcome, Err(RegistrationFailure::Stopped));
                assert_eq!(f.count("registrations"), 0);
                assert_eq!(f.count("registration_intents"), 0);
            }
            _ => unreachable!(),
        }
    }
}

fn hold_ack(f: &Registered, cause: &str) {
    assert!(["registration_cancelled", "drained"].contains(&cause));
    f.store.owner.connection.lock().unwrap().execute_batch(&format!("CREATE TRIGGER hold_cancellation_ack BEFORE UPDATE OF published ON audit WHEN json_extract(OLD.record,'$.cause')='{cause}' BEGIN SELECT RAISE(IGNORE); END;")).unwrap();
}

#[tokio::test]
async fn cancelled_private_intent_stays_removed_through_failed_write_ack_loss_and_cold_replay() {
    for mode in ["expiry", "revoked"] {
        for lost_ack in [false, true] {
            let mut f = Registered::new();
            let (intent, stop) = stopped_reservation(&f, mode).await;
            let held = f.audit.with_extension("held");
            if lost_ack {
                hold_ack(&f, "registration_cancelled");
            } else {
                std::fs::rename(&f.audit, &held).unwrap();
            }
            assert_eq!(
                f.command(&f.store).execute(intent.clone()).await,
                Err(RegistrationFailure::StoppedCleanup(if lost_ack {
                    OutboxError::Conflict
                } else {
                    OutboxError::Unavailable
                }))
            );
            assert_eq!(f.count("registration_intents"), 0);
            assert_eq!(f.count("registrations"), 0);
            let (audit_id, text) = {
                let c = f.store.owner.connection.lock().unwrap();
                let facts = retirement::read(&c, intent.id).unwrap().unwrap();
                assert!(facts.kind == retirement::RetirementKind::Cancelled);
                assert_eq!(facts.stop, stop);
                let text: String = c
                    .query_row(
                        "SELECT record FROM audit WHERE id=?1",
                        [facts.audit.to_string()],
                        |r| r.get(0),
                    )
                    .unwrap();
                if lost_ack {
                    c.execute_batch("DROP TRIGGER hold_cancellation_ack")
                        .unwrap();
                }
                (facts.audit, text)
            };
            if !lost_ack {
                std::fs::rename(&held, &f.audit).unwrap();
            } else {
                assert_eq!(
                    std::fs::read_to_string(f.audit.join(format!("{audit_id}.json"))).unwrap(),
                    text
                );
            }
            if mode == "revoked" {
                let mut changed = serde_json::to_value(SavedStop::of(&stop)).unwrap();
                changed["revocation"]["correlation"] = json!("different-original-command");
                let substituted = SavedStop::decode(&changed.to_string()).unwrap();
                let snapshot = f
                    .auth
                    .read(&CredentialId::new(&intent.issue.credential_id).unwrap())
                    .await
                    .unwrap();
                let oversized_clock = RegistrationStop::revoked(
                    snapshot.revocation_fact().unwrap().unwrap(),
                    u64::MAX,
                );
                for invalid in [substituted, oversized_clock] {
                    let before = files(&f);
                    std::fs::rename(&f.audit, &held).unwrap();
                    assert_eq!(
                        f.store.cancel_uninstalled(intent.clone(), invalid).await,
                        Err(OutboxError::Invalid)
                    );
                    assert!(!f.audit.exists());
                    std::fs::rename(&held, &f.audit).unwrap();
                    assert_eq!(files(&f), before);
                    assert_eq!(
                        f.store
                            .owner
                            .connection
                            .lock()
                            .unwrap()
                            .query_row(
                                "SELECT published FROM audit WHERE id=?1",
                                [audit_id.to_string()],
                                |r| r.get::<_, i64>(0)
                            )
                            .unwrap(),
                        0
                    );
                }
            }
            f.reopen();
            let retry = if mode == "expiry" {
                RegistrationStop::expired(ExpiryObserver::Restart, 200_000)
            } else {
                let snapshot = f
                    .auth
                    .read(&CredentialId::new(&intent.issue.credential_id).unwrap())
                    .await
                    .unwrap();
                RegistrationStop::revoked(snapshot.revocation_fact().unwrap().unwrap(), 200_000)
            };
            assert_eq!(
                f.store.cancel_uninstalled(intent, retry).await,
                Ok(UninstalledCancellation::Cancelled)
            );
            assert_eq!(
                std::fs::read_to_string(f.audit.join(format!("{audit_id}.json"))).unwrap(),
                text
            );
            f.store.due(200_000).await.unwrap();
            assert_eq!(
                f.store
                    .owner
                    .connection
                    .lock()
                    .unwrap()
                    .query_row("SELECT count(*) FROM retirements", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }
}

#[tokio::test]
async fn installed_intent_declines_cancellation_and_keeps_the_existing_stop_owner() {
    let f = Registered::new();
    let intent = f.proposed();
    f.command(&f.store).execute(intent.clone()).await.unwrap();
    f.clock.0.store(100_007, Ordering::SeqCst);
    let stop = RegistrationStop::expired(
        ExpiryObserver::Owner(PrincipalId::new("owner").unwrap()),
        100_007,
    );
    assert_eq!(
        f.store.cancel_uninstalled(intent.clone(), stop).await,
        Ok(UninstalledCancellation::Installed)
    );
    assert_eq!(f.count("registrations"), 1);
    assert_eq!(f.count("registration_intents"), 1);
    assert_eq!(
        f.command(&f.store).execute(intent.clone()).await,
        Err(RegistrationFailure::Stopped)
    );
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 0);
    assert!(!files(&f)
        .values()
        .any(|text| text.contains("registration_cancelled")));
    ObserveRegistrationStop {
        credentials: &f.auth,
        stops: &f.store,
        clock: f.clock.as_ref(),
    }
    .execute(
        &CredentialId::new(&intent.issue.credential_id).unwrap(),
        ExpiryObserver::Restart,
    )
    .await
    .unwrap();
    assert_eq!(f.count("registrations"), 0);
    assert_eq!(f.count("registration_intents"), 0);
    assert!(!files(&f)
        .values()
        .any(|text| text.contains("registration_cancelled")));
}

async fn retained_terminal(
    f: &Registered,
    cancelled: bool,
) -> (RegistrationIntent, String, String) {
    let intent;
    if cancelled {
        (intent, _) = stopped_reservation(f, "expiry").await;
        hold_ack(f, "registration_cancelled");
        assert_eq!(
            f.command(&f.store).execute(intent.clone()).await,
            Err(RegistrationFailure::StoppedCleanup(OutboxError::Conflict))
        );
    } else {
        intent = f.proposed();
        f.command(&f.store).execute(intent.clone()).await.unwrap();
        f.clock.0.store(100_007, Ordering::SeqCst);
        hold_ack(f, "drained");
        assert_eq!(
            ObserveRegistrationStop {
                credentials: &f.auth,
                stops: &f.store,
                clock: f.clock.as_ref()
            }
            .execute(
                &CredentialId::new(&intent.issue.credential_id).unwrap(),
                ExpiryObserver::Owner(PrincipalId::new("owner").unwrap())
            )
            .await,
            Err(StopFailure::Store(OutboxError::Conflict))
        );
    }
    let c = f.store.owner.connection.lock().unwrap();
    c.execute_batch("DROP TRIGGER hold_cancellation_ack")
        .unwrap();
    let (id, text) = c.query_row("SELECT audit.id,audit.record FROM retirements JOIN audit ON audit.id=retirements.audit", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    (intent, id, text)
}

async fn reject_cold_and_live(
    f: &mut Registered,
    live_checks: bool,
    repair: impl FnOnce(&Connection),
) {
    let before = files(f);
    let saved: String = f
        .store
        .owner
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT json_group_array(json_array(id,record,published)) FROM audit",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let held = f.audit.with_extension("held");
    if live_checks {
        std::fs::rename(&f.audit, &held).unwrap();
        assert_eq!(f.store.due(200_000).await.err(), Some(OutboxError::Invalid));
        assert!(!f.audit.exists());
        std::fs::rename(&held, &f.audit).unwrap();
        assert_eq!(files(f), before);
        assert_eq!(
            f.store
                .owner
                .connection
                .lock()
                .unwrap()
                .query_row(
                    "SELECT json_group_array(json_array(id,record,published)) FROM audit",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            saved
        );
    }
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let placeholder = SqlOutbox::open(
        &private.join("outbox.db"),
        private.join("audit"),
        f.clock.clone(),
    )
    .unwrap();
    drop(std::mem::replace(&mut f.store, placeholder));
    std::fs::rename(&f.audit, &held).unwrap();
    assert!(matches!(
        SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
        Err(OutboxError::Invalid)
    ));
    assert!(!f.audit.exists());
    std::fs::rename(&held, &f.audit).unwrap();
    assert_eq!(files(f), before);
    let c = Connection::open(&f.path).unwrap();
    repair(&c);
    drop(c);
    f.store = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
}

async fn crossed_terminal_history(live_checks: bool) {
    for cancelled in [false, true] {
        for field in [
            "kind",
            "projection",
            "extra_owed",
            "extra_published",
            "duplicate",
            "valid",
        ] {
            let mut f = Registered::new();
            let (intent, id, original) = retained_terminal(&f, cancelled).await;
            let other_id = Uuid::new_v4().to_string();
            {
                let c = f.store.owner.connection.lock().unwrap();
                let mut changed: Value = serde_json::from_str(&original).unwrap();
                if field == "duplicate" {
                    changed["recordId"] = json!(other_id);
                } else {
                    changed["cause"] = json!(if cancelled {
                        "drained"
                    } else {
                        "registration_cancelled"
                    });
                    changed["before"] = json!(if cancelled { "stopped" } else { "reserved" });
                }
                audit::Record::decode(&changed.to_string()).unwrap();
                match field {
                    "kind" => {
                        c.execute(
                            "UPDATE retirements SET kind=?1",
                            [if cancelled { "drained" } else { "cancelled" }],
                        )
                        .unwrap();
                    }
                    "projection" => {
                        c.execute(
                            "UPDATE audit SET record=?1 WHERE id=?2",
                            params![changed.to_string(), id],
                        )
                        .unwrap();
                    }
                    "extra_owed" | "extra_published" | "duplicate" => {
                        changed["recordId"] = json!(other_id);
                        if field == "extra_published" {
                            let mut file = nessa_local_storage::open(
                                &f.audit.join(format!("{other_id}.json")),
                                nessa_local_storage::OpenMode::OpenOrCreate,
                            )
                            .unwrap();
                            file.write_all(changed.to_string().as_bytes()).unwrap();
                            file.sync_all().unwrap();
                        }
                        c.execute("INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,?4)", params![other_id,intent.id.to_string(),changed.to_string(),i64::from(field=="extra_published")]).unwrap();
                    }
                    "valid" => {}
                    _ => unreachable!(),
                }
            }
            if field != "valid" {
                reject_cold_and_live(&mut f, live_checks, |c| {
                    c.execute(
                        "UPDATE retirements SET kind=?1",
                        [if cancelled { "cancelled" } else { "drained" }],
                    )
                    .unwrap();
                    c.execute(
                        "UPDATE audit SET record=?1 WHERE id=?2",
                        params![original, id],
                    )
                    .unwrap();
                    c.execute("DELETE FROM audit WHERE id=?1", [&other_id])
                        .unwrap();
                })
                .await;
            } else {
                f.reopen();
            }
            f.store.due(200_000).await.unwrap();
            assert_eq!(
                std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap(),
                original
            );
        }
    }
}

struct WaitingCancellation<'a> {
    store: &'a SqlOutbox,
    reached: Notify,
    release: Notify,
}
#[async_trait]
impl RegistrationIntents for WaitingCancellation<'_> {
    async fn reserve(&self, intent: RegistrationIntent) -> Result<RegistrationIntent, OutboxError> {
        self.store.reserve(intent).await
    }
    async fn install(&self, intent: RegistrationIntent) -> Result<(), OutboxError> {
        RegistrationIntents::install(self.store, intent).await
    }
    async fn discard(&self, id: Uuid) -> Result<(), OutboxError> {
        self.store.discard(id).await
    }
    async fn cancel_uninstalled(
        &self,
        intent: RegistrationIntent,
        stop: RegistrationStop,
    ) -> Result<UninstalledCancellation, OutboxError> {
        self.reached.notify_one();
        self.release.notified().await;
        self.store.cancel_uninstalled(intent, stop).await
    }
}

#[tokio::test]
async fn actual_auth_hold_outlives_cancellation_sink_wait_and_acknowledgement() {
    let f = Registered::new();
    let (intent, _) = stopped_reservation(&f, "expiry").await;
    let waiting = WaitingCancellation {
        store: &f.store,
        reached: Notify::new(),
        release: Notify::new(),
    };
    let command = f.command(&waiting);
    let mut execution = Box::pin(command.execute(intent.clone()));
    tokio::select! { result = &mut execution => panic!("unexpected completion: {result:?}"), () = waiting.reached.notified() => {} }
    let mut revoke = f.auth.revoke(RevokeCredentialRequest {
        request_id: "wait-for-cancellation".into(),
        issuer_principal_id: "owner".into(),
        credential_id: intent.issue.credential_id,
        revoked_at: 101,
    });
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(revoke.as_mut().poll(&mut context), Poll::Pending));
    waiting.release.notify_one();
    assert_eq!(execution.await, Err(RegistrationFailure::Stopped));
    revoke.await.unwrap();
    assert_eq!(f.count("registration_intents"), 0);
}

#[tokio::test]
async fn crossed_terminal_history_refuses_live_publication_before_file_access() {
    crossed_terminal_history(true).await;
}

#[tokio::test]
async fn crossed_terminal_history_refuses_cold_restoration_before_file_access() {
    crossed_terminal_history(false).await;
}

struct SuppliedSnapshot(AccessSnapshot);
impl AccessAdmission for SuppliedSnapshot {
    fn hold<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessHold> {
        Box::pin(async move { Ok(AccessHold::held(self.0.clone(), ())) })
    }
}

#[tokio::test]
async fn absent_or_other_original_fact_cannot_cancel_a_stopped_reservation() {
    for field in ["missing_revocation", "other_revocation", "other_origin"] {
        let f = Registered::new();
        let (intent, _) = stopped_reservation(&f, "revoked").await;
        f.auth
            .recover_owner("replacement-owner".into(), &["credential.manage"], 30, None)
            .unwrap();
        let other = f
            .auth
            .read(&CredentialId::new("owner-key").unwrap())
            .await
            .unwrap();
        let mut snapshot = f
            .auth
            .read(&CredentialId::new(&intent.issue.credential_id).unwrap())
            .await
            .unwrap();
        match field {
            "missing_revocation" => snapshot.revocation = None,
            "other_revocation" => snapshot.revocation = other.revocation,
            "other_origin" => snapshot.origin = other.origin,
            _ => unreachable!(),
        }
        let supplied = SuppliedSnapshot(snapshot); // Deliberately malformed custom adapter.
        let mut command = f.command(&f.store);
        command.admission = &supplied;
        assert_eq!(
            command.execute(intent).await,
            Err(RegistrationFailure::Inconsistent),
            "{field}"
        );
        assert_eq!(f.count("registration_intents"), 1);
        assert_eq!(f.count("registrations"), 0);
        assert!(files(&f).is_empty());
    }
}

#[tokio::test]
async fn disabled_membership_does_not_restore_install_authority_or_block_canonical_cleanup() {
    for mode in ["expiry", "revoked", "current"] {
        let f = Registered::new();
        let intent = if mode == "current" {
            let intent = f.store.reserve(f.proposed()).await.unwrap();
            f.auth.issue_sync(intent.issue.clone()).unwrap();
            intent
        } else {
            stopped_reservation(&f, mode).await.0
        };
        let mut snapshot = f
            .auth
            .read(&CredentialId::new(&intent.issue.credential_id).unwrap())
            .await
            .unwrap();
        let mut disabled = intent.issue.membership.clone();
        disabled.state = MembershipStateDto::Disabled;
        // The custom read authority models a membership disabled after the
        // issue reply. Identity and actual original stop remain unchanged.
        snapshot.membership = nessa_auth::domain::Membership::try_from(disabled).unwrap();
        let supplied = SuppliedSnapshot(snapshot);
        let mut command = f.command(&f.store);
        command.admission = &supplied;
        assert_eq!(
            command.execute(intent).await,
            Err(RegistrationFailure::Stopped)
        );
        assert_eq!(f.count("registrations"), 0);
        assert_eq!(
            f.count("registration_intents"),
            i64::from(mode == "current")
        );
        assert_eq!(
            files(&f).values().any(|text| text.contains("registration_cancelled")),
            mode != "current"
        );
    }
}

#[tokio::test]
async fn retained_cancellation_and_drain_owners_reject_duplicate_credentials_with_distinct_controls(
) {
    for first_cancelled in [false, true] {
        for next_cancelled in [false, true] {
            for conflicting in [false, true] {
                let mut f = Registered::new();
                let (intent, id, original) = retained_terminal(&f, first_cancelled).await;
                let other = Uuid::new_v4().to_string();
                let next_audit = Uuid::new_v4().to_string();
                let credential = if conflicting {
                    intent.issue.credential_id.as_str()
                } else {
                    "distinct-shape-only-key"
                };
                // The second row is an isolated retained-shape control. It grants
                // no Auth authority and is not a claim of a second actual issue.
                let mut record: Value = serde_json::from_str(&original).unwrap();
                record["registration"] = json!(other);
                record["recordId"] = json!(next_audit);
                record["credential"] = json!(credential);
                record["cause"] = json!(if next_cancelled {
                    "registration_cancelled"
                } else {
                    "drained"
                });
                record["before"] = json!(if next_cancelled {
                    "reserved"
                } else {
                    "stopped"
                });
                let text = record.to_string();
                audit::Record::decode(&text).unwrap();
                {
                    let c = f.store.owner.connection.lock().unwrap();
                    c.execute(
                        "INSERT INTO audit(id,registration,record) VALUES(?1,?2,?3)",
                        params![next_audit, other, text],
                    )
                    .unwrap();
                    c.execute("INSERT INTO retirements(id,audit,credential,principal,organization,owner,issued_at,stop,retired_at,kind) SELECT ?1,?2,?3,principal,organization,owner,issued_at,stop,retired_at,?4 FROM retirements WHERE id=?5", params![other,next_audit,credential,if next_cancelled { "cancelled" } else { "drained" },intent.id.to_string()]).unwrap();
                    retirement::read(&c, Uuid::parse_str(&other).unwrap())
                        .unwrap()
                        .unwrap();
                }
                if conflicting {
                    reject_cold_and_live(&mut f, true, |c| {
                        c.execute("DELETE FROM retirements WHERE id=?1", [&other])
                            .unwrap();
                        c.execute("DELETE FROM audit WHERE id=?1", [&next_audit])
                            .unwrap();
                    })
                    .await;
                } else {
                    f.reopen();
                }
                f.store.due(200_000).await.unwrap();
                assert_eq!(
                    std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap(),
                    original
                );
                if !conflicting {
                    assert_eq!(
                        std::fs::read_to_string(f.audit.join(format!("{next_audit}.json")))
                            .unwrap(),
                        text
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn cancelled_and_drained_credentials_cannot_overlap_another_live_registration() {
    for cancelled in [false, true] {
        for conflicting in [false, true] {
            let mut f = Registered::new();
            let mut live = f.proposed();
            live.issue.request_id = "another-live-registration".into();
            live.issue.principal.id = "another-live-hook".into();
            live.issue.membership.id = "another-live-member".into();
            live.issue.membership.principal_id = "another-live-hook".into();
            live.issue.expires_at = Some(200);
            f.command(&f.store).execute(live.clone()).await.unwrap();
            let (intent, id, original) = retained_terminal(&f, cancelled).await;
            let live_id = live.id.to_string();
            if conflicting {
                let guard: String;
                let issue: String;
                {
                    let c = f.store.owner.connection.lock().unwrap();
                    guard = c.query_row("SELECT sql FROM sqlite_schema WHERE name='registration_intent_immutable'", [], |r| r.get(0)).unwrap();
                    issue = c
                        .query_row(
                            "SELECT issue FROM registration_intents WHERE id=?1",
                            [&live_id],
                            |r| r.get(0),
                        )
                        .unwrap();
                    c.execute("DROP TRIGGER registration_intent_immutable", [])
                        .unwrap();
                    c.execute("UPDATE registration_intents SET issue=json_set(issue,'$.credentialId',?1) WHERE id=?2", params![intent.issue.credential_id,live_id]).unwrap();
                    c.execute(
                        "UPDATE registrations SET credential=?1 WHERE id=?2",
                        params![intent.issue.credential_id, live_id],
                    )
                    .unwrap();
                    c.execute_batch(&guard).unwrap();
                    super::super::super::super::intents::validate(&c).unwrap();
                }
                reject_cold_and_live(&mut f, true, |c| {
                    c.execute("DROP TRIGGER registration_intent_immutable", [])
                        .unwrap();
                    c.execute(
                        "UPDATE registration_intents SET issue=?1 WHERE id=?2",
                        params![issue, live_id],
                    )
                    .unwrap();
                    c.execute(
                        "UPDATE registrations SET credential=?1 WHERE id=?2",
                        params![live.issue.credential_id, live_id],
                    )
                    .unwrap();
                    c.execute_batch(&guard).unwrap();
                })
                .await;
            } else {
                f.reopen();
            }
            f.store.due(200_000).await.unwrap();
            assert_eq!(
                f.store
                    .registration(CredentialId::new(&live.issue.credential_id).unwrap())
                    .await
                    .unwrap()
                    .unwrap()
                    .id,
                live.id
            );
            assert_eq!(
                std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap(),
                original
            );
        }
    }
}

async fn four_fact_history(live_checks: bool) {
    for duplicate in [
        None,
        Some("credential_expired"),
        Some("credential_expiry_observed"),
    ] {
        let mut f = Registered::new();
        let (intent, id, original) = retained_terminal(&f, false).await;
        let original_files = files(&f);
        let mut lifecycle = None;
        {
            let c = f.store.owner.connection.lock().unwrap();
            for text in original_files.values() {
                let record: Value = serde_json::from_str(text).unwrap();
                let cause = record["cause"].as_str().unwrap();
                if ["credential_issue", "credential_expiry_observed"].contains(&cause) {
                    // Restore actual earlier acknowledged projections. Their
                    // exact canonical durable files are still present.
                    c.execute(
                        "INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,1)",
                        params![
                            record["recordId"].as_str().unwrap(),
                            intent.id.to_string(),
                            text
                        ],
                    )
                    .unwrap();
                }
                if duplicate == Some(cause) {
                    lifecycle = Some(record);
                }
            }
            assert_eq!(
                c.query_row("SELECT count(*) FROM audit", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                4
            );
            super::super::super::super::restoration::validate(&c).unwrap();
        }
        let extra = Uuid::new_v4().to_string();
        if let Some(mut record) = lifecycle {
            record["recordId"] = json!(extra);
            let text = record.to_string();
            audit::Record::decode(&text).unwrap();
            let mut file = nessa_local_storage::open(
                &f.audit.join(format!("{extra}.json")),
                nessa_local_storage::OpenMode::OpenOrCreate,
            )
            .unwrap();
            file.write_all(text.as_bytes()).unwrap();
            file.sync_all().unwrap();
            f.store
                .owner
                .connection
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,1)",
                    params![extra, intent.id.to_string(), text],
                )
                .unwrap();
            reject_cold_and_live(&mut f, live_checks, |c| {
                c.execute("DELETE FROM audit WHERE id=?1", [&extra])
                    .unwrap();
            })
            .await;
        } else {
            assert!(duplicate.is_none());
            f.reopen();
        }
        f.store.due(200_000).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap(),
            original
        );
    }
}

#[async_trait]
impl crate::webhooks::application::RegistrationStops for WaitingCancellation<'_> {
    async fn stop_credential(
        &self,
        credential: CredentialId,
        stop: RegistrationStop,
    ) -> Result<(), OutboxError> {
        self.reached.notify_one();
        self.release.notified().await;
        self.store.stop_credential(credential, stop).await
    }
}

#[tokio::test]
async fn installed_stop_keeps_the_same_auth_hold_and_original_command_reader() {
    let f = Registered::new();
    let intent = f.proposed();
    f.command(&f.store).execute(intent.clone()).await.unwrap();
    f.clock.0.store(100_007, Ordering::SeqCst);
    let waiting = WaitingCancellation {
        store: &f.store,
        reached: Notify::new(),
        release: Notify::new(),
    };
    let mut command = f.command(&f.store);
    command.stops = &waiting;
    let mut execution = Box::pin(command.execute(intent.clone()));
    tokio::select! { result = &mut execution => panic!("unexpected completion: {result:?}"), () = waiting.reached.notified() => {} }
    let mut revoke = f.auth.revoke(RevokeCredentialRequest {
        request_id: "wait-for-installed-stop".into(),
        issuer_principal_id: "owner".into(),
        credential_id: intent.issue.credential_id,
        revoked_at: 101,
    });
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(revoke.as_mut().poll(&mut context), Poll::Pending));
    waiting.release.notify_one();
    assert_eq!(execution.await, Err(RegistrationFailure::Stopped));
    revoke.await.unwrap();
    assert_eq!(f.count("registrations"), 0);
    let observed: Vec<Value> = files(&f)
        .values()
        .map(|text| serde_json::from_str(text).unwrap())
        .collect();
    assert!(observed
        .iter()
        .any(|record| record["cause"] == "credential_expiry_observed"
            && record["initiator"] == "owner:owner"));
    assert!(!observed
        .iter()
        .any(|record| record["cause"] == "registration_cancelled"));
}

#[tokio::test]
async fn complete_retired_history_and_duplicate_lifecycle_facts_are_checked_before_live_publication(
) {
    four_fact_history(true).await;
}

#[tokio::test]
async fn complete_retired_history_and_duplicate_lifecycle_facts_are_checked_before_cold_restore() {
    four_fact_history(false).await;
}

#[tokio::test]
async fn only_a_drained_registration_can_retain_a_published_initial_projection() {
    for cancelled in [false, true] {
        let mut f = Registered::new();
        let (intent, id, original) = retained_terminal(&f, cancelled).await;
        let (initial_id, initial) = if cancelled {
            // Deliberately fabricated installation evidence: the actual Auth
            // issue exists, but this destination was never installed.
            let record = audit::Record::registration(
                &intent.registration().unwrap(),
                audit::RegistrationChange::Issue,
                intent.observed_at_ms,
            );
            (
                record.keys().unwrap().0.to_string(),
                record.encode().unwrap(),
            )
        } else {
            files(&f)
                .values()
                .find_map(|text| {
                    let record: Value = serde_json::from_str(text).unwrap();
                    (record["cause"] == "credential_issue").then(|| {
                        (
                            record["recordId"].as_str().unwrap().to_owned(),
                            text.clone(),
                        )
                    })
                })
                .unwrap()
        };
        if cancelled {
            let mut file = nessa_local_storage::open(
                &f.audit.join(format!("{initial_id}.json")),
                nessa_local_storage::OpenMode::OpenOrCreate,
            )
            .unwrap();
            file.write_all(initial.as_bytes()).unwrap();
            file.sync_all().unwrap();
        }
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,1)",
                params![initial_id, intent.id.to_string(), initial],
            )
            .unwrap();
        if cancelled {
            reject_cold_and_live(&mut f, true, |c| {
                c.execute("DELETE FROM audit WHERE id=?1", [&initial_id])
                    .unwrap();
            })
            .await;
        } else {
            f.reopen();
        }
        f.store.due(200_000).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap(),
            original
        );
    }
}
