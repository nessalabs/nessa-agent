//! SQL work and ownership checks; full-boundary lifecycle fixtures stay intact.
use super::*;
use crate::webhooks::infrastructure::outbox::journal::{TRIM_REGISTRATION_AUDIT, TRIM_TERMINALS};
use nessa_local_database::rusqlite::StatementStatus;

// These are deliberately SQL metadata fixtures, not an authenticated replay or
// audit-publication fixture. Real full-boundary tests exercise those owners.
async fn terminal_metadata(count: usize) -> Fixture {
    let f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued = f.store.due(1000).await.unwrap().remove(0).delivery;
    f.store
        .transition(
            &queued,
            &queued.refused().unwrap(),
            DeliveryCause::Refused,
            1001,
        )
        .await
        .unwrap();
    {
        let mut c = f.store.owner.connection.lock().unwrap();
        let tx = c.transaction().unwrap();
        for position in 2..=count {
            let id = Uuid::new_v4().to_string();
            tx.execute("INSERT INTO deliveries(id,registration,conversation,incarnation,position,scope,body,state) SELECT ?1,registration,conversation,incarnation,?2,scope,body,json_set(state,'$.id',?1) FROM deliveries WHERE id=?3", params![id,position as i64,queued.id().to_string()]).unwrap();
        }
        tx.commit().unwrap();
    }
    f
}

#[tokio::test]
async fn terminal_cleanup_walks_one_indexed_cutoff_per_registration() {
    for count in [128, 512, RETAINED_LIMIT] {
        let f = terminal_metadata(count).await;
        // A full pending backlog must not make terminal selection visit
        // nonterminal rows. This makes the partial-index requirement observable.
        {
            let mut c = f.store.owner.connection.lock().unwrap();
            let source = c
                .query_row(
                    "SELECT id FROM deliveries ORDER BY rowid LIMIT 1",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .unwrap();
            let tx = c.transaction().unwrap();
            for position in count + 1..=count + PENDING_LIMIT {
                let id = Uuid::new_v4().to_string();
                tx.execute("INSERT INTO deliveries(id,registration,conversation,incarnation,position,scope,body,state) SELECT ?1,registration,conversation,incarnation,?2,scope,body,json_set(state,'$.id',?1,'$.phase','queued') FROM deliveries WHERE id=?3", params![id,position as i64,source]).unwrap();
            }
            tx.commit().unwrap();
        }
        let c = f.store.owner.connection.lock().unwrap();
        let mut query = c.prepare(TRIM_TERMINALS).unwrap();
        assert_eq!(query.execute([RETAINED_LIMIT as i64 - 1]).unwrap(), 0);
        let steps = query.get_status(StatementStatus::VmStep);
        assert!(
            steps <= 20 * count as i32 + 512,
            "count={count} VM steps={steps}"
        );
        let plan = c
            .prepare(&format!("EXPLAIN QUERY PLAN {TRIM_TERMINALS}"))
            .unwrap()
            .query_map([RETAINED_LIMIT as i64 - 1], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plan.iter().any(|line| line.contains("terminal_deliveries")),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|line| line.contains("TEMP B-TREE")),
            "{plan:?}"
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM deliveries", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            (count + PENDING_LIMIT) as i64
        );
    }
}

#[tokio::test]
async fn registration_cleanup_does_not_visit_published_delivery_history() {
    let f = terminal_metadata(1).await;
    let mut c = f.store.owner.connection.lock().unwrap();
    let delivery = c
        .query_row("SELECT id FROM deliveries", [], |r| r.get::<_, String>(0))
        .unwrap();
    let mut baseline = None;
    for count in [128, 8192] {
        let tx = c.transaction().unwrap();
        tx.execute("DELETE FROM audit", []).unwrap();
        for _ in 0..count {
            tx.execute("INSERT INTO audit(id,registration,delivery,record,published) VALUES(?1,?2,?3,'{}',1)", params![Uuid::new_v4().to_string(),f.registration.id.to_string(),delivery]).unwrap();
        }
        tx.commit().unwrap();
        let mut query = c.prepare(TRIM_REGISTRATION_AUDIT).unwrap();
        assert_eq!(query.execute([]).unwrap(), 0);
        let steps = query.get_status(StatementStatus::VmStep);
        if let Some(before) = baseline {
            assert!(
                steps <= before + 16,
                "published history changed cleanup work: {before} -> {steps}"
            );
        }
        baseline = Some(steps);
        assert_eq!(
            c.query_row("SELECT count(*) FROM audit", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            count
        );
    }
}

#[tokio::test]
async fn delivery_retirement_removes_only_its_published_audit_and_preserves_owed_rows() {
    let f = terminal_metadata(RETAINED_LIMIT + 1).await;
    let c = f.store.owner.connection.lock().unwrap();
    let oldest = c
        .query_row(
            "SELECT id FROM deliveries ORDER BY rowid LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    let owed = Uuid::new_v4().to_string();
    c.execute(
        "INSERT INTO audit(id,registration,delivery,record,published) VALUES(?1,?2,?3,'{}',0)",
        params![owed, f.registration.id.to_string(), oldest],
    )
    .unwrap();
    assert_eq!(
        c.execute(TRIM_TERMINALS, [RETAINED_LIMIT as i64 - 1])
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM deliveries", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        RETAINED_LIMIT as i64 + 1
    );
    c.execute("UPDATE audit SET published=1 WHERE id=?1", [&owed])
        .unwrap();
    assert_eq!(
        c.execute(TRIM_TERMINALS, [RETAINED_LIMIT as i64 - 1])
            .unwrap(),
        1
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM deliveries", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        RETAINED_LIMIT as i64
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM audit WHERE delivery=?1",
            [&oldest],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    // If a raw corrupt writer bypasses the retirement owner, owed evidence is
    // retained for restore refusal rather than erased by the cleanup trigger.
    let next = c
        .query_row(
            "SELECT id FROM deliveries ORDER BY rowid LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    c.execute(
        "INSERT INTO audit(id,registration,delivery,record,published) VALUES(?1,?2,?3,'{}',0)",
        params![
            Uuid::new_v4().to_string(),
            f.registration.id.to_string(),
            next
        ],
    )
    .unwrap();
    c.execute("DELETE FROM deliveries WHERE id=?1", [&next])
        .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM audit WHERE delivery=?1 AND published=0",
            [&next],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn deleting_nonterminal_metadata_does_not_erase_its_acknowledged_history() {
    let f = Fixture::new().await;
    let record = f.record(1);
    let id = record.id.to_string();
    f.store.queue(record).await.unwrap();
    let c = f.store.owner.connection.lock().unwrap();
    c.execute("DELETE FROM deliveries WHERE id=?1", [&id])
        .unwrap();
    assert!(
        c.query_row(
            "SELECT count(*) FROM audit WHERE delivery=?1 AND published=1",
            [&id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
            > 0
    );
    assert_eq!(
        crate::webhooks::infrastructure::outbox::restoration::validate(&c),
        Err(OutboxError::Invalid)
    );
}

#[tokio::test]
async fn queued_lifecycle_selection_does_not_walk_terminal_history() {
    use crate::webhooks::infrastructure::outbox::restoration::QUEUED_REGISTRATIONS;
    let mut baseline = None;
    for count in [1, RETAINED_LIMIT] {
        let f = terminal_metadata(count).await;
        let c = f.store.owner.connection.lock().unwrap();
        let source: String = c
            .query_row("SELECT id FROM deliveries LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let id = Uuid::new_v4().to_string();
        c.execute("INSERT INTO deliveries(id,registration,conversation,incarnation,position,scope,body,state) SELECT ?1,registration,conversation,incarnation,?2,scope,body,json_set(state,'$.id',?1,'$.phase','queued') FROM deliveries WHERE id=?3", params![id, (count + 1) as i64, source]).unwrap();
        let mut q = c.prepare(QUEUED_REGISTRATIONS).unwrap();
        let ids = q
            .query_map(
                [(crate::webhooks::application::MAX_REGISTRATIONS + 1) as i64],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(ids, [f.registration.id.to_string()]);
        let steps = q.get_status(StatementStatus::VmStep);
        if let Some(before) = baseline {
            assert_eq!(steps, before);
        } else {
            baseline = Some(steps);
        }
        assert!(steps < 128, "steps={steps}");
    }
}

#[tokio::test]
async fn queued_lifecycle_selection_refuses_excess_distinct_registration_work() {
    // Metadata-only work-bound fixture, not authenticated/restorable history.
    let f = terminal_metadata(1).await;
    let c = f.store.owner.connection.lock().unwrap();
    let original = f.registration.id.to_string();
    let source: String = c
        .query_row("SELECT id FROM deliveries LIMIT 1", [], |r| r.get(0))
        .unwrap();
    for _ in 0..=crate::webhooks::application::MAX_REGISTRATIONS {
        let registration = Uuid::new_v4().to_string();
        let delivery = Uuid::new_v4().to_string();
        c.execute("INSERT INTO registration_intents(id,issuer,request,issue,destination,mac_key,observed_at) SELECT ?1,issuer,?1,issue,destination,mac_key,observed_at FROM registration_intents WHERE id=?2", params![registration, original]).unwrap();
        c.execute("INSERT INTO registrations(id,credential,principal,organization,owner,issued_at,destination,mac_key,active) SELECT ?1,?1,?1,organization,owner,issued_at,destination,mac_key,1 FROM registrations WHERE id=?2", params![registration,original]).unwrap();
        c.execute("INSERT INTO deliveries(id,registration,conversation,incarnation,position,scope,body,state) SELECT ?1,?2,conversation,incarnation,1,scope,body,json_set(state,'$.id',?1,'$.phase','queued') FROM deliveries WHERE id=?3", params![delivery,registration,source]).unwrap();
    }
    assert_eq!(restoration::validate_queued(&c), Err(OutboxError::Invalid));
}
