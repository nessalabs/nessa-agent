//! Complete but contradictory ownership must refuse before files or cleanup.
use super::*;

async fn retained(f: &Fixture) -> (String, String) {
    f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER hold_retirement_ack BEFORE UPDATE OF published ON audit WHEN json_extract(OLD.record,'$.cause')='drained' BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    assert_eq!(
        f.store
            .stop_credential(
                f.registration.target.credential.clone(),
                RegistrationStop::expired(ExpiryObserver::Restart, 2000),
            )
            .await,
        Err(OutboxError::Conflict)
    );
    let c = f.store.owner.connection.lock().unwrap();
    c.execute_batch("DROP TRIGGER hold_retirement_ack").unwrap();
    c.query_row(
        "SELECT audit,record FROM retirements JOIN audit ON audit.id=retirements.audit",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

fn snapshot(c: &Connection) -> Vec<String> {
    [
        "SELECT json_group_array(json_array(id,credential,active,stop)) FROM registrations",
        "SELECT json_group_array(json_array(id,audit,credential,stop)) FROM retirements",
        "SELECT json_group_array(json_array(id,record,published)) FROM audit",
    ]
    .iter()
    .map(|sql| c.query_row(sql, [], |r| r.get(0)).unwrap())
    .collect()
}

async fn refuse_then_repair(f: &mut Fixture, live_checks: bool, repair: impl FnOnce(&Connection)) {
    let before = snapshot(&f.store.owner.connection.lock().unwrap());
    let files = std::fs::read_dir(&f.audit).unwrap().count();
    if live_checks {
        assert_eq!(f.store.due(3000).await.err(), Some(OutboxError::Invalid));
        assert_eq!(snapshot(&f.store.owner.connection.lock().unwrap()), before);
        assert_eq!(std::fs::read_dir(&f.audit).unwrap().count(), files);
    }
    assert_eq!(reopen_store(f).await.err(), Some(OutboxError::Invalid));
    assert_eq!(std::fs::read_dir(&f.audit).unwrap().count(), files);
    let c = Connection::open(&f.path).unwrap();
    assert_eq!(snapshot(&c), before);
    repair(&c);
    drop(c);
    f.store = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
}

async fn duplicate_retired_credentials(live_checks: bool) {
    for conflicting in [true, false] {
        let mut f = Fixture::new().await;
        let (original_id, original_text) = retained(&f).await;
        // The actual file exists while its SQL acknowledgement is lost.
        assert_eq!(
            std::fs::read_to_string(f.audit.join(format!("{original_id}.json"))).unwrap(),
            original_text
        );
        let other = Uuid::new_v4().to_string();
        let audit_id = Uuid::new_v4().to_string();
        let credential = if conflicting {
            f.registration.target.credential.as_str()
        } else {
            "distinct-retired-key"
        };
        let mut record: Value = serde_json::from_str(&original_text).unwrap();
        record["registration"] = json!(other);
        record["recordId"] = json!(audit_id);
        record["credential"] = json!(credential);
        let text = record.to_string();
        {
            let c = f.store.owner.connection.lock().unwrap();
            c.execute(
                "INSERT INTO audit(id,registration,record) VALUES(?1,?2,?3)",
                params![audit_id, other, text],
            )
            .unwrap();
            c.execute("INSERT INTO retirements(id,audit,credential,principal,organization,owner,issued_at,stop,retired_at) SELECT ?1,?2,?3,principal,organization,owner,issued_at,stop,retired_at FROM retirements WHERE id=?4",params![other,audit_id,credential,f.registration.id.to_string()]).unwrap();
            // Each row/record is independently valid; only ownership conflicts.
            audit::Record::decode(&text).unwrap();
            assert!(
                super::super::super::retirement::read(&c, Uuid::parse_str(&other).unwrap())
                    .unwrap()
                    .is_some()
            );
        }
        if conflicting {
            refuse_then_repair(&mut f, live_checks, |c| {
                c.execute("DELETE FROM retirements WHERE id=?1", [&other])
                    .unwrap();
                c.execute("DELETE FROM audit WHERE id=?1", [&audit_id])
                    .unwrap();
            })
            .await;
            assert!(!f.audit.join(format!("{audit_id}.json")).exists());
        } else {
            f.store = reopen_store(&mut f).await.unwrap();
        }
        assert!(f.store.due(3000).await.unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(f.audit.join(format!("{original_id}.json"))).unwrap(),
            original_text
        );
        if !conflicting {
            assert_eq!(
                std::fs::read_to_string(f.audit.join(format!("{audit_id}.json"))).unwrap(),
                text
            );
        }
    }
}

async fn live_and_retired_credentials(live_checks: bool) {
    for conflicting in [true, false] {
        let mut f = Fixture::new().await;
        let mut live = f.registration.clone();
        live.id = Uuid::new_v4();
        live.target.credential = CredentialId::new("distinct-live-key").unwrap();
        live.target.principal = PrincipalId::new("distinct-live-principal").unwrap();
        f.store.install(live.clone(), 1000).await.unwrap();
        let (original_id, original_text) = retained(&f).await;
        if conflicting {
            let id = live.id.to_string();
            let original_issue: String;
            let immutable: String;
            {
                let c = f.store.owner.connection.lock().unwrap();
                immutable = c
                    .query_row(
                        "SELECT sql FROM sqlite_schema WHERE name='registration_intent_immutable'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                original_issue = c
                    .query_row(
                        "SELECT issue FROM registration_intents WHERE id=?1",
                        [&id],
                        |r| r.get(0),
                    )
                    .unwrap();
                c.execute("DROP TRIGGER registration_intent_immutable", [])
                    .unwrap();
                c.execute("UPDATE registration_intents SET issue=json_set(issue,'$.credentialId',?1) WHERE id=?2", params![f.registration.target.credential.as_str(),id]).unwrap();
                c.execute(
                    "UPDATE registrations SET credential=?1 WHERE id=?2",
                    params![f.registration.target.credential.as_str(), id],
                )
                .unwrap();
                c.execute_batch(&immutable).unwrap();
                super::super::super::intents::validate(&c).unwrap();
            }
            refuse_then_repair(&mut f, live_checks, |c| {
                c.execute("DROP TRIGGER registration_intent_immutable", [])
                    .unwrap();
                c.execute(
                    "UPDATE registration_intents SET issue=?1 WHERE id=?2",
                    params![original_issue, id],
                )
                .unwrap();
                c.execute(
                    "UPDATE registrations SET credential=?1 WHERE id=?2",
                    params![live.target.credential.as_str(), id],
                )
                .unwrap();
                c.execute_batch(&immutable).unwrap();
            })
            .await;
        } else {
            f.store = reopen_store(&mut f).await.unwrap();
        }
        assert!(f.store.due(3000).await.unwrap().is_empty());
        assert_eq!(
            f.store
                .registration(live.target.credential.clone())
                .await
                .unwrap()
                .unwrap()
                .id,
            live.id
        );
        assert_eq!(
            std::fs::read_to_string(f.audit.join(format!("{original_id}.json"))).unwrap(),
            original_text
        );
    }
}

#[tokio::test]
async fn duplicate_retirements_refuse_publication_without_trimming() {
    duplicate_retired_credentials(true).await;
}

#[tokio::test]
async fn duplicate_retirements_refuse_cold_restore_without_publication() {
    duplicate_retired_credentials(false).await;
}

#[tokio::test]
async fn live_retired_overlap_refuses_publication_without_trimming() {
    live_and_retired_credentials(true).await;
}

#[tokio::test]
async fn live_retired_overlap_refuses_cold_restore_without_publication() {
    live_and_retired_credentials(false).await;
}
