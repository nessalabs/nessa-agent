//! Coherent acknowledged prehistory, not a substitute for threshold effects.
//! One representative delivery uses the real public durability path. Remaining
//! past rows use canonical domain transitions/audit builders in one fixture
//! transaction, with every corresponding private audit file actually written
//! and the audit directory synced once. Per-file fsync is deliberately omitted
//! here: this bulk prehistory is setup, no retention test simulates power loss
//! (every "restart"/"cold" case is an in-process reopen), and the up-to-6144
//! individual FlushFileBuffers calls cost over two minutes per fixture call on
//! Windows CI, which overran the server-other step budget (#1203).
//! Retention tests still invoke the real crossing/reset/stop and recovery paths.
use super::super::super::audit::Record;
use super::super::super::{capacity, codec, intents, journal, restoration, source_reset, stored};
use super::Fixture;
use crate::webhooks::application::{DeliveryCause, Outbox, PENDING_LIMIT, RETAINED_LIMIT};
use crate::webhooks::domain::{Delivery, Observation};
use nessa_local_database::rusqlite::{params, Transaction};
use nessa_local_storage::OpenMode;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::time::Instant;

pub(super) async fn acknowledged_prehistory(
    f: &Fixture,
    terminal: usize,
    pending: usize,
    sent: bool,
) {
    assert!((1..=RETAINED_LIMIT).contains(&terminal));
    assert!(pending <= PENDING_LIMIT);
    let start = Instant::now();
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    if sent {
        let sending = queued.admit(7, 2000).unwrap();
        f.store
            .transition(&queued, &sending, DeliveryCause::Admission, 2000)
            .await
            .unwrap();
        f.store
            .transition(
                &sending,
                &sending.observed(Observation::Success, 2001).unwrap(),
                DeliveryCause::Response,
                2001,
            )
            .await
            .unwrap();
    } else {
        f.store
            .transition(
                &queued,
                &queued.refused().unwrap(),
                DeliveryCause::Refused,
                1001,
            )
            .await
            .unwrap();
    }
    let mut c = f.store.owner.connection.lock().unwrap();
    let tx = c.transaction().unwrap();
    for position in 2..=(terminal + pending) as u64 {
        let record = f.record(position);
        let queued = Delivery::queued(record.id, record.epoch);
        tx.execute("INSERT INTO deliveries(id,registration,conversation,incarnation,position,scope,body,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![record.id.to_string(),record.registration.to_string(),record.conversation.to_string(),record.scope.incarnation().as_str(),position as i64,codec::scope_text(&record.scope).unwrap(),record.body,codec::encode(&queued).unwrap()]).unwrap();
        journal::delivery_audit(
            &tx,
            &queued,
            None,
            DeliveryCause::SourceQueued {
                initiator: record.initiator,
            },
            record.observed_at_ms,
        )
        .unwrap();
        if position <= terminal as u64 {
            if sent {
                let sending = queued.admit(7, 2000).unwrap();
                replace(&tx, &queued, &sending, DeliveryCause::Admission, 2000);
                replace(
                    &tx,
                    &sending,
                    &sending.observed(Observation::Success, 2001).unwrap(),
                    DeliveryCause::Response,
                    2001,
                );
            } else {
                replace(
                    &tx,
                    &queued,
                    &queued.refused().unwrap(),
                    DeliveryCause::Refused,
                    1001,
                );
            }
        }
    }
    tx.execute(
        "UPDATE cursors SET position=?1 WHERE registration=?2 AND conversation=?3",
        params![
            (terminal + pending) as i64,
            f.registration.id.to_string(),
            f.conversation.to_string()
        ],
    )
    .unwrap();
    // These represent already acknowledged history. Materialize the exact
    // canonical projections, write every file and sync their directory, then
    // commit their published flags with the fixture rows. No production
    // publisher, flush, or acknowledgement code is changed or bypassed by the
    // actual operation that follows this setup.
    let records = {
        let mut query = tx
            .prepare("SELECT id,record FROM audit WHERE published=0 ORDER BY rowid")
            .unwrap();
        query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert!(records.len() <= 3 * (RETAINED_LIMIT + PENDING_LIMIT));
    for (id, text) in records {
        let record = Record::decode(&text).unwrap();
        assert_eq!(record.keys().unwrap().0.to_string(), id);
        let mut file =
            nessa_local_storage::open(&f.audit.join(format!("{id}.json")), OpenMode::CreateNew)
                .unwrap();
        file.write_all(text.as_bytes()).unwrap();
        drop(file);
        assert_eq!(
            tx.execute(
                "UPDATE audit SET published=1 WHERE id=?1 AND published=0",
                [id]
            )
            .unwrap(),
            1
        );
    }
    nessa_local_storage::sync_directory(&f.audit).unwrap();
    intents::validate(&tx).unwrap();
    source_reset::validate_all(&tx).unwrap();
    stored::validate(&tx).unwrap();
    restoration::validate(&tx).unwrap();
    capacity::check(&tx, Some(f.registration.id)).unwrap();
    tx.commit().unwrap();
    eprintln!(
        "retention prehistory terminal={terminal} pending={pending} sent={sent} setup={:?}",
        start.elapsed()
    );
}
fn replace(
    tx: &Transaction<'_>,
    before: &Delivery,
    after: &Delivery,
    cause: DeliveryCause,
    at: u64,
) {
    assert_eq!(
        tx.execute(
            "UPDATE deliveries SET state=?1 WHERE id=?2 AND state=?3",
            params![
                codec::encode(after).unwrap(),
                before.id().to_string(),
                codec::encode(before).unwrap()
            ]
        )
        .unwrap(),
        1
    );
    journal::delivery_audit(tx, after, Some(before), cause, at).unwrap();
}

#[tokio::test]
async fn acknowledged_fixture_matches_real_public_history_and_durable_projections() {
    for sent in [false, true] {
        let actual = Fixture::new().await;
        for position in 1..=2 {
            actual.store.queue(actual.record(position)).await.unwrap();
            let queued = actual.store.due(1000).await.unwrap().remove(0).delivery;
            if sent {
                let sending = queued.admit(7, 2000).unwrap();
                actual
                    .store
                    .transition(&queued, &sending, DeliveryCause::Admission, 2000)
                    .await
                    .unwrap();
                actual
                    .store
                    .transition(
                        &sending,
                        &sending.observed(Observation::Success, 2001).unwrap(),
                        DeliveryCause::Response,
                        2001,
                    )
                    .await
                    .unwrap();
            } else {
                actual
                    .store
                    .transition(
                        &queued,
                        &queued.refused().unwrap(),
                        DeliveryCause::Refused,
                        1001,
                    )
                    .await
                    .unwrap();
            }
        }
        for position in 3..=4 {
            actual.store.queue(actual.record(position)).await.unwrap();
        }
        let fixture = Fixture::new().await;
        acknowledged_prehistory(&fixture, 2, 2, sent).await;
        assert_eq!(snapshot(&fixture), snapshot(&actual), "sent={sent}");
    }
}

// Normalize only fresh identities. All state, payload, scope, target, causal
// before/after, initiator, time and acknowledgement values remain compared.
fn snapshot(f: &Fixture) -> Value {
    let c = f.store.owner.connection.lock().unwrap();
    let mut identities = BTreeMap::new();
    identities.insert(f.registration.id.to_string(), "registration".to_owned());
    identities.insert(f.conversation.to_string(), "conversation".to_owned());
    let deliveries = {
        let mut q = c
            .prepare("SELECT id,position FROM deliveries ORDER BY position")
            .unwrap();
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    for (id, position) in deliveries {
        identities.insert(id, format!("delivery-{position}"));
    }
    let mut files = f.audits();
    for file in &files {
        let name = format!(
            "audit-{}-{}",
            file["position"],
            file["cause"].as_str().unwrap()
        );
        identities.insert(file["recordId"].as_str().unwrap().to_owned(), name);
    }
    let mut rows = {
        let mut q = c
            .prepare("SELECT id,position,phase,state,scope,body FROM deliveries ORDER BY position")
            .unwrap();
        q.query_map([],|r|{
            let state:String=r.get(3)?; let scope:String=r.get(4)?;
            Ok(json!({"id":r.get::<_,String>(0)?,"position":r.get::<_,i64>(1)?,"phase":r.get::<_,String>(2)?,"state":serde_json::from_str::<Value>(&state).unwrap(),"scope":serde_json::from_str::<Value>(&scope).unwrap(),"body":r.get::<_,Vec<u8>>(5)?}))
        }).unwrap().collect::<Result<Vec<_>,_>>().unwrap()
    };
    let mut audits = {
        let mut q = c
            .prepare("SELECT record,published FROM audit ORDER BY rowid")
            .unwrap();
        q.query_map([],|r|{let text:String=r.get(0)?;Ok(json!({"record":serde_json::from_str::<Value>(&text).unwrap(),"published":r.get::<_,i64>(1)?}))}).unwrap().collect::<Result<Vec<_>,_>>().unwrap()
    };
    let cursor: i64 = c
        .query_row("SELECT position FROM cursors", [], |r| r.get(0))
        .unwrap();
    for value in rows
        .iter_mut()
        .chain(audits.iter_mut())
        .chain(files.iter_mut())
    {
        normalize(value, &identities);
    }
    files.sort_by_key(Value::to_string);
    json!({"rows":rows,"audit":audits,"files":files,"cursor":cursor})
}
fn normalize(value: &mut Value, identities: &BTreeMap<String, String>) {
    match value {
        Value::String(s) => {
            for (from, to) in identities {
                *s = s.replace(from, to);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize(item, identities);
            }
        }
        Value::Object(fields) => {
            for item in fields.values_mut() {
                normalize(item, identities);
            }
        }
        _ => {}
    }
}
