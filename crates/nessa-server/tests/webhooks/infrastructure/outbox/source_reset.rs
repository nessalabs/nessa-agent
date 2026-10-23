use super::*;
use crate::webhooks::application::{MAX_CURSORS, MAX_SOURCE_RESETS, SOURCE_RESET_BYTES};
use nessa_local_database::rusqlite::{types::Type, StatementStatus};

async fn captured_sender(f: &Fixture) -> Delivery {
    f.store.queue(f.record(1)).await.unwrap();
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
    sending
}

fn current_reset(f: &Fixture) -> Uuid {
    let id: String = f
        .store
        .owner
        .connection
        .lock()
        .unwrap()
        .query_row("SELECT source_reset FROM cursors", [], |r| r.get(0))
        .unwrap();
    Uuid::parse_str(&id).unwrap()
}

#[tokio::test]
async fn sending_and_cursor_share_original_immutable_reset_and_membership() {
    let mut f = Fixture::new().await;
    let sending = captured_sender(&f).await;
    let id = current_reset(&f);
    {
        let c = f.store.owner.connection.lock().unwrap();
        let fact = source_reset::read(&c, id).unwrap().unwrap();
        assert_eq!(fact.stop.initiator, QueueInitiator::Live);
        assert_eq!(fact.stop.at_ms, 1002);
        assert!(c.execute("UPDATE source_reset_facts SET fact=json_set(fact,'$.stop.initiator','restart','$.stop.at_ms',9999) WHERE id=?1",[id.to_string()]).is_err());
        assert!(c
            .execute(
                "UPDATE source_reset_members SET reset=?1",
                [Uuid::new_v4().to_string()]
            )
            .is_err());
        assert!(c
            .execute("UPDATE source_reset_members SET reset=reset", [])
            .is_err());
        assert!(c.execute("DELETE FROM source_reset_members", []).is_err());
        assert_eq!(
            c.query_row(
                "SELECT reset FROM source_reset_members WHERE delivery=?1",
                [sending.id().to_string()],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            id.to_string()
        );
    }
    let reopened = reopen_store(&mut f).await.unwrap();
    reopened.recover(3000).await.unwrap();
    let record = f
        .audits()
        .into_iter()
        .find(|a| a["cause"] == "restart")
        .unwrap();
    assert_eq!(record["sourceResetId"], id.to_string());
    assert_eq!(record["sourceStop"]["initiator"], "live");
    assert_eq!(record["sourceStop"]["at_ms"], 1002);
    assert_eq!(record["initiator"], "restart");
    assert_eq!(record["after"], "unknown");
}

#[tokio::test]
async fn a_capture_without_a_reset_cannot_be_inserted_or_recovered() {
    let mut f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    {
        let c = f.store.owner.connection.lock().unwrap();
        assert!(c.execute("INSERT INTO source_reset_members(delivery,reset,position,state) VALUES(?1,?2,1,?3)",params![sending.id().to_string(),Uuid::new_v4().to_string(),codec::encode(&sending).unwrap()]).is_err());
        assert_eq!(
            c.query_row("SELECT count(*) FROM source_reset_members", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    let reopened = reopen_store(&mut f).await.unwrap();
    reopened.recover(3000).await.unwrap();
    let record = f
        .audits()
        .into_iter()
        .find(|a| a["cause"] == "restart")
        .unwrap();
    assert!(record["sourceResetId"].is_null());
    assert!(record["sourceStop"].is_null());
}

#[tokio::test]
async fn immediate_source_refusal_uses_original_reset_actor_time_and_identity() {
    for field in [
        "initiator",
        "atMs",
        "sourceStopActor",
        "sourceStopTime",
        "sourceResetId",
        "scope",
    ] {
        let mut f = Fixture::new().await;
        f.store.queue(f.record(1)).await.unwrap();
        let held = f.audit.with_extension("held");
        std::fs::rename(&f.audit, &held).unwrap();
        assert_eq!(
            f.store.queue(replaced_source(f.record(2))).await,
            Err(OutboxError::Unavailable)
        );
        std::fs::rename(&held, &f.audit).unwrap();
        let id = edit_evidence(&f, "webhook_delivery", "source_changed", |r| match field {
            "initiator" => r[field] = json!("restart"),
            "atMs" => r[field] = json!(9999),
            "sourceStopActor" => {
                r["sourceStop"]["initiator"] = json!("restart");
                r["initiator"] = json!("restart");
            }
            "sourceStopTime" => r["sourceStop"]["at_ms"] = json!(9999),
            "sourceResetId" => r[field] = json!(Uuid::new_v4().to_string()),
            "scope" => r[field]["origin"] = json!("another-valid-origin"),
            _ => unreachable!(),
        });
        rejects_evidence_before_live_or_restored_publication(&mut f, &id).await;
    }
}

#[tokio::test]
async fn history_cannot_claim_reset_before_the_original_capture_state() {
    let mut f = Fixture::new().await;
    captured_sender(&f).await;
    let reset = current_reset(&f);
    let id = edit_evidence(&f, "webhook_delivery", "admission", |r| {
        r["sourceResetId"] = json!(reset.to_string());
        r["sourceStop"] = json!({"initiator":"live","at_ms":1002});
    });
    // Keep the audit FK projection consistent, so only causal order rejects it.
    f.store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute(
            "UPDATE audit SET source_reset=?1 WHERE id=?2",
            params![reset.to_string(), id],
        )
        .unwrap();
    rejects_evidence_before_live_or_restored_publication(&mut f, &id).await;
}

#[tokio::test]
async fn first_reset_survives_later_cursor_deletion_replacement_and_restart() {
    let mut f = Fixture::new().await;
    let original = captured_sender(&f).await;
    let first = current_reset(&f);
    assert!(f.store.due(3000).await.unwrap().is_empty());
    f.store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute("DELETE FROM cursors", [])
        .unwrap();
    let mut new = f.record(1);
    new.scope = replaced_source(f.record(1)).scope;
    f.store.queue(new).await.unwrap();
    let mut next = replaced_source(f.record(2));
    next.scope = Scope::new(
        next.scope.receiver().clone(),
        next.scope.origin().clone(),
        next.scope.stream().clone(),
        Id::new("third-incarnation").unwrap(),
        next.scope.schema().clone(),
        next.scope.access_epoch().clone(),
    );
    next.initiator = QueueInitiator::Restart;
    assert_eq!(f.store.queue(next).await, Err(OutboxError::ResetRequired));
    let second = current_reset(&f);
    assert_ne!(first, second);
    let reopened = reopen_store(&mut f).await.unwrap();
    reopened.recover(4000).await.unwrap();
    let record = f
        .audits()
        .into_iter()
        .find(|a| a["cause"] == "restart" && a["deliveryId"] == original.id().to_string())
        .unwrap();
    assert_eq!(record["sourceResetId"], first.to_string());
    assert_eq!(record["sourceStop"]["initiator"], "live");
    assert_eq!(record["sourceStop"]["at_ms"], 1002);
    let c = reopened.owner.connection.lock().unwrap();
    assert_eq!(
        c.query_row("SELECT source_reset FROM cursors", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        second.to_string()
    );
}

#[tokio::test]
async fn a_reset_may_capture_older_logical_epochs_of_the_same_physical_source() {
    let mut f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    let mut newer = f.record(2);
    newer.epoch = 8;
    newer.scope = Scope::new(
        newer.scope.receiver().clone(),
        newer.scope.origin().clone(),
        newer.scope.stream().clone(),
        newer.scope.incarnation().clone(),
        newer.scope.schema().clone(),
        Id::new("epoch-8").unwrap(),
    );
    f.store.queue(newer).await.unwrap();
    let mut changed = replaced_source(f.record(3));
    changed.epoch = 8;
    changed.scope = Scope::new(
        changed.scope.receiver().clone(),
        changed.scope.origin().clone(),
        changed.scope.stream().clone(),
        changed.scope.incarnation().clone(),
        changed.scope.schema().clone(),
        Id::new("epoch-8").unwrap(),
    );
    assert_eq!(
        f.store.queue(changed).await,
        Err(OutboxError::ResetRequired)
    );
    let id = current_reset(&f);
    let reopened = reopen_store(&mut f).await.unwrap();
    reopened.recover(4000).await.unwrap();
    let c = reopened.owner.connection.lock().unwrap();
    let fact = source_reset::read(&c, id).unwrap().unwrap();
    assert_eq!(fact.prior_scope.access_epoch().as_str(), "epoch-8");
    let saved = stored::by_id(&c, sending.id()).unwrap().unwrap();
    assert_eq!(saved.delivery.epoch(), 7);
    assert_eq!(saved.source_reset, Some(id));
}

#[tokio::test]
async fn reset_references_release_with_retirement_and_lost_ack_recovery() {
    let mut f = Fixture::new().await;
    let sending = captured_sender(&f).await;
    let id = current_reset(&f);
    f.store.due(3000).await.unwrap();
    {
        let c = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            c.query_row("SELECT refs FROM source_reset_facts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        c.execute("DELETE FROM cursors", []).unwrap();
        assert_eq!(
            c.query_row("SELECT refs FROM source_reset_facts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(source_reset::read(&c, id).unwrap().is_some());
    }
    let sent = sending.observed(Observation::Success, 3100).unwrap();
    f.store
        .transition(&sending, &sent, DeliveryCause::Response, 3100)
        .await
        .unwrap();
    f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER hold_retire_ack BEFORE UPDATE OF published ON audit WHEN json_extract(OLD.record,'$.cause')='drained' BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    assert_eq!(
        f.store
            .stop(
                f.registration.target.credential.clone(),
                StopReason::Revoked {
                    initiator: Initiator::Principal(
                        PrincipalId::new("original-retirement-actor").unwrap()
                    ),
                    cause: RevocationCause::Explicit
                },
                3200
            )
            .await,
        Err(OutboxError::Conflict)
    );
    {
        let c = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM source_reset_facts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM source_reset_members", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM registrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        c.execute_batch("DROP TRIGGER hold_retire_ack").unwrap();
    }
    let reopened = reopen_store(&mut f).await.unwrap();
    reopened.due(4000).await.unwrap();
    let retired = f
        .audits()
        .into_iter()
        .find(|a| a["cause"] == "drained")
        .unwrap();
    assert_eq!(retired["initiator"], "principal:original-retirement-actor");
}

#[test]
fn owed_selection_work_does_not_grow_with_published_history() {
    let mut previous = None;
    for count in [0, 100, 10000] {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        let tx = c.transaction().unwrap();
        // Selector-only SQL fixture, not fabricated domain restoration evidence.
        for i in 0..=count {
            tx.execute("INSERT INTO audit(id,registration,record,published) VALUES(?1,'synthetic','{}',?2)",params![i.to_string(),i!=count]).unwrap();
        }
        tx.commit().unwrap();
        let mut q = c.prepare(restoration::audit_selection(true)).unwrap();
        let rows = q
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(q.get_status(StatementStatus::FullscanStep), 0);
        let steps = q.get_status(StatementStatus::VmStep);
        if let Some(prior) = previous {
            assert!(steps <= prior + 2);
        }
        previous = Some(steps);
    }
}

#[tokio::test]
async fn missing_references_and_reference_accounting_refuse_restore_before_effects() {
    for corruption in [
        "missing_fact",
        "member_reference",
        "member_state",
        "member_identity",
        "member_epoch",
        "member_attempt",
        "member_timestamp",
        "member_position",
        "counter",
    ] {
        let mut f = Fixture::new().await;
        let captured = captured_sender(&f).await;
        let files = || {
            let mut paths = std::fs::read_dir(&f.audit)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            paths.sort();
            paths
                .into_iter()
                .map(|p| (p.file_name().unwrap().to_owned(), std::fs::read(p).unwrap()))
                .collect::<Vec<_>>()
        };
        let original_files = files();
        {
            let c = f.store.owner.connection.lock().unwrap();
            c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
            match corruption {
                "missing_fact" => {
                    c.execute("DELETE FROM source_reset_facts", []).unwrap();
                }
                "counter" => {
                    c.execute("UPDATE source_reset_facts SET refs=refs+1", [])
                        .unwrap();
                }
                "member_reference" | "member_state" | "member_identity" | "member_epoch"
                | "member_attempt" | "member_timestamp" | "member_position" => {
                    let guard:String=c.query_row("SELECT sql FROM sqlite_schema WHERE name='source_reset_member_immutable'",[],|r|r.get(0)).unwrap();
                    c.execute_batch("DROP TRIGGER source_reset_member_immutable")
                        .unwrap();
                    if corruption == "member_reference" {
                        c.execute(
                            "UPDATE source_reset_members SET reset=?1",
                            [Uuid::new_v4().to_string()],
                        )
                        .unwrap();
                    } else if corruption == "member_position" {
                        c.execute("UPDATE source_reset_members SET position=position+1", [])
                            .unwrap();
                    } else if corruption != "member_state" {
                        let (path, value) = match corruption {
                            "member_identity" => ("$.id", json!(Uuid::new_v4().to_string())),
                            "member_epoch" => ("$.epoch", json!(8)),
                            "member_attempt" => ("$.attempts", json!(2)),
                            "member_timestamp" => ("$.timestamp_ms", json!(2001)),
                            _ => unreachable!(),
                        };
                        c.execute(
                            "UPDATE source_reset_members SET state=json_set(state,?1,json(?2))",
                            params![path, value.to_string()],
                        )
                        .unwrap();
                    } else {
                        c.execute("UPDATE source_reset_members SET state='invalid'", [])
                            .unwrap();
                    }
                    c.execute_batch(&guard).unwrap();
                }
                _ => unreachable!(),
            }
            c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            if corruption.starts_with("member_") {
                assert!(
                    matches!(stored::by_id(&c, captured.id()), Err(OutboxError::Invalid)),
                    "{corruption}"
                );
            }
        }
        assert_eq!(reopen_store(&mut f).await.err(), Some(OutboxError::Invalid));
        let mut paths = std::fs::read_dir(&f.audit)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        paths.sort();
        let after = paths
            .into_iter()
            .map(|p| (p.file_name().unwrap().to_owned(), std::fs::read(p).unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(original_files, after);
    }
}

#[tokio::test]
async fn missing_or_replaced_immutable_guards_are_not_the_current_schema() {
    for guard in [
        "source_reset_fact_immutable",
        "source_reset_member_immutable",
        "source_reset_member_retention",
        "source_reset_cursor_release",
    ] {
        let mut f = Fixture::new().await;
        captured_sender(&f).await;
        f.store.owner.connection.lock().unwrap().execute_batch(&format!("DROP TRIGGER {guard}; CREATE TRIGGER {guard} BEFORE UPDATE ON source_reset_facts BEGIN SELECT 1; END;")).unwrap();
        assert_eq!(reopen_store(&mut f).await.err(), Some(OutboxError::Invalid));
    }
}

#[tokio::test]
async fn failed_capture_rolls_back_fact_members_cursor_and_evidence() {
    let mut f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(2000).await.unwrap().remove(0).delivery;
    let sending = queued.admit(7, 2000).unwrap();
    f.store
        .transition(&queued, &sending, DeliveryCause::Admission, 2000)
        .await
        .unwrap();
    f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER fail_capture BEFORE INSERT ON source_reset_members BEGIN SELECT RAISE(ABORT,'synthetic capture failure'); END;").unwrap();
    assert_eq!(
        f.store.queue(replaced_source(f.record(2))).await,
        Err(OutboxError::Conflict)
    );
    {
        let c = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM source_reset_facts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM source_reset_members", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            c.query_row("SELECT reset_required FROM cursors", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM audit WHERE source_reset IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            stored::by_id(&c, sending.id()).unwrap().unwrap().delivery,
            sending
        );
        c.execute_batch("DROP TRIGGER fail_capture").unwrap();
    }
    let reopened = reopen_store(&mut f).await.unwrap();
    reopened.recover(4000).await.unwrap();
    assert!(f
        .audits()
        .into_iter()
        .filter(|a| a["cause"] == "restart")
        .all(|a| a["sourceStop"].is_null()));
}

#[tokio::test]
async fn cursor_capacity_refuses_new_chat_without_partial_queue_or_reset() {
    assert_eq!(MAX_CURSORS, 1024);
    let mut f = Fixture::new().await;
    {
        let mut c = f.store.owner.connection.lock().unwrap();
        let tx = c.transaction().unwrap();
        // Bounded cursor-only history fixture; no authorization/E2E claim.
        for _ in 0..MAX_CURSORS {
            let chat = Uuid::new_v4().to_string();
            let scope = f.record(1).scope;
            let scope = Scope::new(
                scope.receiver().clone(),
                scope.origin().clone(),
                Id::new(&chat).unwrap(),
                scope.incarnation().clone(),
                scope.schema().clone(),
                scope.access_epoch().clone(),
            );
            tx.execute(
                "INSERT INTO cursors(registration,conversation,scope,position) VALUES(?1,?2,?3,1)",
                params![
                    f.registration.id.to_string(),
                    chat,
                    codec::scope_text(&scope).unwrap()
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER reject_delivery_insert BEFORE INSERT ON deliveries BEGIN SELECT RAISE(ABORT,'cursor capacity must refuse first'); END;").unwrap();
    assert_eq!(f.store.queue(f.record(1)).await, Err(OutboxError::Capacity));
    f.store
        .owner
        .connection
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_delivery_insert")
        .unwrap();
    {
        let c = f.store.owner.connection.lock().unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM deliveries", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM source_reset_facts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        c.execute(
            "INSERT INTO cursors(registration,conversation,scope,position) VALUES(?1,?2,?3,1)",
            params![
                f.registration.id.to_string(),
                f.conversation.to_string(),
                codec::scope_text(&f.record(1).scope).unwrap()
            ],
        )
        .unwrap();
    }
    assert_eq!(reopen_store(&mut f).await.err(), Some(OutboxError::Invalid));
}

#[tokio::test]
async fn reset_factory_count_capacity_does_not_insert_a_partial_fact() {
    assert_eq!(MAX_SOURCE_RESETS, 3072);
    let f = Fixture::new().await;
    let mut c = f.store.owner.connection.lock().unwrap();
    let tx = c.transaction().unwrap();
    // Isolate the factory's count guard with typed, audit-referenced facts.
    // This intentionally does not claim an assembled queue/cleanup history.
    for _ in 0..MAX_SOURCE_RESETS {
        let fact = source_reset::Fact {
            id: Uuid::new_v4(),
            registration: f.registration.id,
            conversation: f.conversation.clone(),
            prior_scope: f.record(1).scope,
            prior_position: 1,
            observed_scope: replaced_source(f.record(2)).scope,
            stop: codec::SourceStop {
                initiator: QueueInitiator::Live,
                at_ms: 1002,
            },
        };
        source_reset::insert(&tx, &fact).unwrap();
        publication::append(&tx, audit::Record::cursor(&fact)).unwrap();
    }
    let fact = source_reset::Fact {
        id: Uuid::new_v4(),
        registration: f.registration.id,
        conversation: f.conversation.clone(),
        prior_scope: f.record(1).scope,
        prior_position: 1,
        observed_scope: replaced_source(f.record(2)).scope,
        stop: codec::SourceStop {
            initiator: QueueInitiator::Live,
            at_ms: 1002,
        },
    };
    assert_eq!(source_reset::insert(&tx, &fact), Err(OutboxError::Capacity));
    assert_eq!(
        tx.query_row("SELECT count(*) FROM source_reset_facts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        MAX_SOURCE_RESETS as i64
    );
    assert!(source_reset::read(&tx, fact.id).unwrap().is_none());
    tx.rollback().unwrap();
}

#[tokio::test]
async fn oversized_fact_is_not_materialized_before_typed_decoding() {
    assert_eq!(SOURCE_RESET_BYTES, 16 * 1024);
    let f = Fixture::new().await;
    let mut c = f.store.owner.connection.lock().unwrap();
    let tx = c.transaction().unwrap();
    for bytes in [SOURCE_RESET_BYTES, SOURCE_RESET_BYTES + 1] {
        let id = Uuid::new_v4();
        let raw = " ".repeat(bytes);
        tx.execute(
            "INSERT INTO source_reset_facts(id,registration,fact) VALUES(?1,?2,?3)",
            params![id.to_string(), f.registration.id.to_string(), raw],
        )
        .unwrap();
        // Exact owned read query, measured before fetching a Rust String.
        let kind = tx
            .query_row(
                source_reset::READ,
                params![id.to_string(), SOURCE_RESET_BYTES as i64],
                |row| Ok(row.get_ref(1)?.data_type()),
            )
            .unwrap();
        assert_eq!(
            kind,
            if bytes > SOURCE_RESET_BYTES {
                Type::Null
            } else {
                Type::Text
            }
        );
        assert!(matches!(
            source_reset::read(&tx, id),
            Err(OutboxError::Invalid)
        ));
    }
    tx.rollback().unwrap();
}

#[tokio::test]
async fn reset_metadata_substitutions_reject_before_recovery_publication() {
    for field in [
        "id",
        "registration",
        "conversation",
        "actor",
        "time",
        "prior_scope",
        "observed_scope",
        "position",
    ] {
        let mut f = Fixture::new().await;
        captured_sender(&f).await;
        let reset = current_reset(&f);
        if field == "position" {
            // Historical cursor evidence still owns the original position after
            // its mutable cursor projection has been removed.
            f.store
                .owner
                .connection
                .lock()
                .unwrap()
                .execute("DELETE FROM cursors", [])
                .unwrap();
        }
        let original = f.audits();
        {
            let c = f.store.owner.connection.lock().unwrap();
            let guard: String = c
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name='source_reset_fact_immutable'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            c.execute_batch("DROP TRIGGER source_reset_fact_immutable")
                .unwrap();
            let (path, value) = match field {
                "id" | "registration" | "conversation" => {
                    (format!("$.{field}"), json!(Uuid::new_v4().to_string()))
                }
                "actor" => ("$.stop.initiator".into(), json!("restart")),
                "time" => ("$.stop.at_ms".into(), json!(9999)),
                "prior_scope" | "observed_scope" => {
                    (format!("$.{field}.origin"), json!("substituted-origin"))
                }
                "position" => ("$.prior_position".into(), json!(2)),
                _ => unreachable!(),
            };
            // SQLite preserves field order. Bypass only the immutable guard to
            // model a damaged file, then restore the original schema before open.
            c.execute(
                "UPDATE source_reset_facts SET fact=json_set(fact,?1,json(?2))",
                params![path, value.to_string()],
            )
            .unwrap();
            if field == "id" {
                assert!(matches!(
                    source_reset::read(&c, reset),
                    Err(OutboxError::Invalid)
                ));
            }
            c.execute_batch(&guard).unwrap();
        }
        assert_eq!(
            reopen_store(&mut f).await.err(),
            Some(OutboxError::Invalid),
            "{field}"
        );
        assert_eq!(f.audits(), original);
    }
}

#[tokio::test]
async fn reset_fact_decoder_rejects_noncanonical_or_invalid_original_values() {
    let f = Fixture::new().await;
    captured_sender(&f).await;
    let c = f.store.owner.connection.lock().unwrap();
    let text: String = c
        .query_row("SELECT fact FROM source_reset_facts", [], |r| r.get(0))
        .unwrap();
    let mut fact = source_reset::Fact::decode(&text).unwrap();
    fact.stop.at_ms = u64::MAX;
    assert!(matches!(fact.encode(), Err(OutboxError::Invalid)));
    let mut fact = source_reset::Fact::decode(&text).unwrap();
    fact.prior_position = u64::MAX;
    assert!(matches!(fact.encode(), Err(OutboxError::Invalid)));
    assert_eq!(SOURCE_RESET_BYTES, 16384);
    assert_eq!(MAX_SOURCE_RESETS, 3072);
    assert!(source_reset::Fact::decode(&text).is_ok());
    for (path, value) in [
        ("$.stop.at_ms", json!(u64::MAX)),
        ("$.prior_position", json!(u64::MAX)),
        ("$.stop.initiator", json!("unknown-reader")),
        ("$.id", json!("not-an-id")),
        ("$.prior_scope.receiver", json!(Uuid::new_v4().to_string())),
        ("$.observed_scope.stream", json!(Uuid::new_v4().to_string())),
    ] {
        let changed: String = c
            .query_row(
                "SELECT json_set(?1,?2,json(?3))",
                params![text, path, value.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            matches!(
                source_reset::Fact::decode(&changed),
                Err(OutboxError::Invalid)
            ),
            "{path}"
        );
    }
    assert!(matches!(
        source_reset::Fact::decode(&format!(" {text}")),
        Err(OutboxError::Invalid)
    ));
    assert!(matches!(
        source_reset::Fact::decode(&"x".repeat(SOURCE_RESET_BYTES + 1)),
        Err(OutboxError::Invalid)
    ));
}

#[tokio::test]
async fn capture_storage_failure_remains_unavailable_instead_of_conflict() {
    let f = Fixture::new().await;
    let delivery = f.store.queue(f.record(1)).await.unwrap();
    let mut c = f.store.owner.connection.lock().unwrap();
    let tx = c.transaction().unwrap();
    let saved = stored::by_id(&tx, delivery).unwrap().unwrap();
    let fact = source_reset::Fact {
        id: Uuid::new_v4(),
        registration: f.registration.id,
        conversation: f.conversation.clone(),
        prior_scope: saved.scope.clone(),
        prior_position: 1,
        observed_scope: replaced_source(f.record(2)).scope,
        stop: codec::SourceStop {
            initiator: QueueInitiator::Live,
            at_ms: 1002,
        },
    };
    source_reset::insert(&tx, &fact).unwrap();
    tx.execute_batch("PRAGMA query_only=ON").unwrap();
    assert_eq!(
        source_reset::capture(&tx, &fact, &saved),
        Err(OutboxError::Unavailable)
    );
    tx.rollback().unwrap();
    c.execute_batch("PRAGMA query_only=OFF").unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM source_reset_facts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
