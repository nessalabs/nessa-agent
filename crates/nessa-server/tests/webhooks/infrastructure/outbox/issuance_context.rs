//! Initial acknowledgement cannot be resurrected after retained later effects.
use super::*;

fn insert_issue(f: &Fixture, issue: &Value, published: bool) {
    let c = f.store.owner.connection.lock().unwrap();
    c.execute(
        "INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,?4)",
        params![
            issue["recordId"].as_str().unwrap(),
            f.registration.id.to_string(),
            issue.to_string(),
            i64::from(published)
        ],
    )
    .unwrap();
}

fn retained(f: &Fixture) -> Vec<String> {
    let c = f.store.owner.connection.lock().unwrap();
    [
        "SELECT json_group_array(json_array(id,active,stop_cause,stop_initiator,stop_at)) FROM registrations",
        "SELECT json_group_array(json_array(id,state)) FROM deliveries",
        "SELECT json_group_array(json_array(id,record,published)) FROM audit",
        "SELECT json_group_array(json_array(id,audit)) FROM retirements",
        "SELECT json_group_array(json_array(registration,conversation,scope,position,reset_required)) FROM cursors",
    ].iter().map(|s| c.query_row(s, [], |r| r.get(0)).unwrap()).collect()
}

async fn later_history(kind: usize, reader: ExpiryObserver) -> (Fixture, Value) {
    let f = Fixture::new().await;
    let issue = f
        .audits()
        .into_iter()
        .find(|v| v["cause"] == "credential_issue")
        .unwrap();
    match kind {
        0 => f
            .store
            .observe_expiry(f.registration.target.credential.clone(), reader, 2001)
            .await
            .unwrap(),
        1 => {
            f.store.queue(f.record(1)).await.unwrap();
            let queued = f.store.due(1001).await.unwrap().remove(0).delivery;
            let sending = queued.admit(7, 2000).unwrap();
            f.store
                .transition(&queued, &sending, DeliveryCause::Admission, 2000)
                .await
                .unwrap();
            f.store
                .stop(
                    f.registration.target.credential.clone(),
                    StopReason::Expired {
                        observed_by: reader,
                    },
                    2001,
                )
                .await
                .unwrap();
        }
        2 | 3 => {
            f.store.queue(f.record(1)).await.unwrap();
            {
                let queued = f.store.due(1001).await.unwrap().remove(0).delivery;
                let refused = queued.refused().unwrap();
                f.store
                    .transition(&queued, &refused, DeliveryCause::Refused, 2000)
                    .await
                    .unwrap();
            }
            if kind == 2 {
                f.store
                    .owner
                    .connection
                    .lock()
                    .unwrap()
                    .execute("DELETE FROM deliveries", [])
                    .unwrap();
            } else {
                f.store
                    .owner
                    .connection
                    .lock()
                    .unwrap()
                    .execute("DELETE FROM cursors", [])
                    .unwrap();
            }
        }
        4 => {
            let held = f.audit.with_extension("held");
            std::fs::rename(&f.audit, &held).unwrap();
            assert_eq!(
                f.store
                    .stop(
                        f.registration.target.credential.clone(),
                        StopReason::Revoked {
                            initiator: f.registration.target.owner.clone(),
                        },
                        2001,
                    )
                    .await,
                Err(OutboxError::Unavailable)
            );
            std::fs::rename(held, &f.audit).unwrap();
        }
        _ => unreachable!(),
    }
    // Establish valid adjacent retained state independently of the new refusal.
    let c = f.store.owner.connection.lock().unwrap();
    assert_eq!(restoration::validate(&c), Ok(()));
    drop(c);
    (f, issue)
}

#[tokio::test]
async fn completed_issue_cannot_be_owed_after_later_retained_work() {
    for kind in 0..5 {
        for reader in [
            ExpiryObserver::Owner(PrincipalId::new("original-reader").unwrap()),
            ExpiryObserver::Delivery(Uuid::new_v4()),
            ExpiryObserver::Restart,
        ] {
            let (mut f, issue) = later_history(kind, reader).await;
            insert_issue(&f, &issue, false);
            let before = retained(&f);
            let held = f.audit.with_extension("held");
            std::fs::rename(&f.audit, &held).unwrap();
            assert_eq!(f.store.due(3000).await.err(), Some(OutboxError::Invalid));
            assert_eq!(retained(&f), before);
            assert!(!f.audit.exists());
            std::fs::rename(held, &f.audit).unwrap();
            assert!(matches!(
                reopen_store(&mut f).await,
                Err(OutboxError::Invalid)
            ));
            let original_bytes = std::fs::read(&f.path).unwrap();
            let files = f.audits().len();
            assert!(matches!(
                SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()),
                Err(OutboxError::Invalid)
            ));
            assert_eq!(std::fs::read(&f.path).unwrap(), original_bytes);
            assert_eq!(f.audits().len(), files);
            let c = Connection::open(&f.path).unwrap();
            c.execute(
                "DELETE FROM audit WHERE id=?1",
                [issue["recordId"].as_str().unwrap()],
            )
            .unwrap();
            drop(c);
            f.store = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
            f.store.due(3000).await.unwrap();
        }
    }
}

#[tokio::test]
async fn published_original_issue_remains_valid_with_later_history() {
    for kind in 0..5 {
        let (mut f, issue) = later_history(kind, ExpiryObserver::Restart).await;
        insert_issue(&f, &issue, true);
        let original = reopen_store(&mut f).await.unwrap();
        f.store = original;
        f.store.due(3000).await.unwrap();
    }
}

#[tokio::test]
async fn genuine_initial_issue_lost_publication_ack_recovers_before_work() {
    for file_written in [false, true] {
        let mut f = Fixture::new().await;
        // Another registration's retained work does not prove this one's Issue
        // has acknowledged. The relationship is scoped by stable identity.
        f.store.queue(f.record(1)).await.unwrap();
        let mut registration = f.registration.clone();
        registration.id = Uuid::new_v4();
        registration.target.credential = CredentialId::new("new-key").unwrap();
        registration.target.principal = PrincipalId::new("new-hook").unwrap();
        let held = f.audit.with_extension("held");
        if file_written {
            f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER ignore_issue_ack BEFORE UPDATE OF published ON audit BEGIN SELECT RAISE(IGNORE); END;").unwrap();
        } else {
            std::fs::rename(&f.audit, &held).unwrap();
        }
        assert_eq!(
            f.store.install(registration.clone(), 1000).await,
            Err(if file_written {
                OutboxError::Conflict
            } else {
                OutboxError::Unavailable
            })
        );
        f.registration = registration;
        let c = f.store.owner.connection.lock().unwrap();
        assert_eq!(restoration::validate(&c), Ok(()));
        assert_eq!(
            c.query_row("SELECT count(*) FROM audit WHERE published=0", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(c);
        if file_written {
            assert!(f
                .audits()
                .iter()
                .any(|v| v["registration"] == f.registration.id.to_string()));
            f.store
                .owner
                .connection
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER ignore_issue_ack")
                .unwrap();
        } else {
            std::fs::rename(held, &f.audit).unwrap();
        }
        f.store = reopen_store(&mut f).await.unwrap();
        f.store.queue(f.record(1)).await.unwrap();
        assert_eq!(f.store.due(1001).await.unwrap().len(), 2);
        assert_eq!(
            f.audits()
                .iter()
                .filter(|v| v["cause"] == "credential_issue"
                    && v["registration"] == f.registration.id.to_string())
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn issuance_work_proof_uses_registration_keys_at_retained_capacity() {
    use nessa_local_database::rusqlite::StatementStatus;
    let mut baseline = None;
    for count in [1, RETAINED_LIMIT] {
        let f = Fixture::new().await;
        let mut c = f.store.owner.connection.lock().unwrap();
        let registration = f.registration.id.to_string();
        // SQL selection metadata only; no permission or restorable history is
        // claimed for these bounded query-plan inputs.
        let tx = c.transaction().unwrap();
        for i in 0..count {
            tx.execute("INSERT INTO cursors(registration,conversation,scope,position) VALUES(?2,?1,'{}',0)", params![i.to_string(),registration]).unwrap();
            tx.execute("INSERT INTO deliveries(id,registration,conversation,incarnation,position,scope,body,state) VALUES(?1,?3,'chat','incarnation',?2,'{}',X'00','{\"phase\":\"terminal\",\"due_at\":0}')", params![i.to_string(), i as i64+1,registration]).unwrap();
        }
        tx.commit().unwrap();
        let mut q = c.prepare(restoration::ISSUANCE_WORK).unwrap();
        assert!(!q.query_row(["missing"], |r| r.get::<_, bool>(0)).unwrap());
        assert_eq!(q.get_status(StatementStatus::FullscanStep), 0);
        let steps = q.get_status(StatementStatus::VmStep);
        assert!(steps < 64, "steps={steps}");
        if let Some(previous) = baseline {
            assert_eq!(steps, previous);
        }
        baseline = Some(steps);
        assert!(q
            .query_row([&registration], |r| r.get::<_, bool>(0))
            .unwrap());
        c.execute("DELETE FROM cursors", []).unwrap();
        assert!(q
            .query_row([&registration], |r| r.get::<_, bool>(0))
            .unwrap());
    }
}
