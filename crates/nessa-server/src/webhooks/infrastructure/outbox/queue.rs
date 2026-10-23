//! One transaction advances the contiguous source cursor with its delivery.
use super::journal::{commit, delivery_audit, invalidate_source};
use super::publication::{append, publish};
use super::stored::{self, StoredDelivery};
use super::{codec, integer, invalid, unavailable};
use crate::webhooks::application::{DeliveryCause, OutboxError, QueuedRecord, MAX_BODY_BYTES};
use crate::webhooks::domain::Delivery;
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;
use uuid::Uuid;

pub(super) fn queue(
    connection: &mut Connection,
    audit: &Path,
    record: QueuedRecord,
) -> Result<Uuid, OutboxError> {
    if record.body.len() > MAX_BODY_BYTES
        || record.position == 0
        || record.expected_after.checked_add(1) != Some(record.position)
        || record.scope.access_epoch().as_str() != format!("epoch-{}", record.epoch)
    {
        return Err(OutboxError::Invalid);
    }
    let registration = record.registration.to_string();
    let conversation = record.conversation.to_string();
    stored::check_cursor(&registration, &conversation, &record.scope)?;
    let scope = codec::scope_text(&record.scope)?;
    let position = integer(record.position)?;
    let transaction = connection.transaction().map_err(unavailable)?;
    let active: Option<i64> = transaction
        .query_row(
            "SELECT active FROM registrations WHERE id=?1",
            [&registration],
            |r| r.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    if active != Some(1) {
        return Err(OutboxError::Conflict);
    }
    let existing:Option<(StoredDelivery,Vec<u8>)>=transaction.query_row(
        &format!("{},body FROM deliveries WHERE registration=?1 AND conversation=?2 AND incarnation=?3 AND position=?4",stored::SELECT),
        params![registration,conversation,record.scope.incarnation().as_str(),position],
        |r|Ok((StoredDelivery::row(r)?,r.get(10)?)),
    ).optional().map_err(unavailable)?;
    if let Some((saved, body)) = existing {
        let existing = saved.checked()?;
        if codec::scope_text(&existing.scope)? != scope
            || body != record.body
            || existing.delivery.epoch() != record.epoch
        {
            return Err(OutboxError::Conflict);
        }
        return Ok(existing.delivery.id());
    }
    let cursor:Option<(String,i64,i64)>=transaction.query_row(
        "SELECT scope,position,reset_required FROM cursors WHERE registration=?1 AND conversation=?2",
        params![registration,conversation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional().map_err(unavailable)?;
    if let Some((old, after, reset)) = &cursor {
        let old = codec::scope(old)?;
        stored::check_cursor(&registration, &conversation, &old)?;
        if *reset != 0 {
            return Err(OutboxError::ResetRequired);
        }
        if !codec::same_source(&old, &record.scope) {
            let changed=transaction.execute(
                "UPDATE cursors SET reset_required=1 WHERE registration=?1 AND conversation=?2 AND reset_required=0",
                params![registration,conversation],
            ).map_err(unavailable)?;
            if changed != 1 {
                return Err(OutboxError::Conflict);
            }
            invalidate_source(
                &transaction,
                record.registration,
                &record.conversation,
                &old,
                codec::SourceStop {
                    initiator: record.initiator,
                    at_ms: record.observed_at_ms,
                },
            )?;
            append(
                &transaction,
                record.registration,
                None,
                json!({
                    "kind":"webhook_cursor","registration":registration,"conversationId":conversation,
                    "before":"following","after":"reset_required",
                    "priorScope":serde_json::from_str::<Value>(&codec::scope_text(&old)?).map_err(invalid)?,
                    "priorPosition":after,"observedScope":serde_json::from_str::<Value>(&scope).map_err(invalid)?,
                    "cause":"source_changed","initiator":record.initiator.label(record.registration),"atMs":record.observed_at_ms
                }),
            )?;
            commit(
                transaction,
                Some(record.registration),
                record.observed_at_ms,
            )?;
            publish(connection, audit)?;
            return Err(OutboxError::ResetRequired);
        }
        if *after != integer(record.expected_after)? {
            return Err(OutboxError::Conflict);
        }
    } else if record.expected_after != 0 {
        return Err(OutboxError::Conflict);
    }
    let delivery = Delivery::queued(record.id, record.epoch);
    transaction.execute(
        "INSERT INTO deliveries(id,registration,conversation,incarnation,position,scope,body,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![record.id.to_string(),registration,conversation,record.scope.incarnation().as_str(),position,scope,record.body,codec::encode(&delivery)?],
    ).map_err(unavailable)?;
    transaction.execute(
        "INSERT INTO cursors(registration,conversation,scope,position) VALUES(?1,?2,?3,?4) ON CONFLICT(registration,conversation) DO UPDATE SET scope=excluded.scope,position=excluded.position",
        params![registration,conversation,scope,position],
    ).map_err(unavailable)?;
    delivery_audit(
        &transaction,
        &delivery,
        None,
        DeliveryCause::SourceQueued {
            initiator: record.initiator,
        },
        record.observed_at_ms,
    )?;
    commit(
        transaction,
        Some(record.registration),
        record.observed_at_ms,
    )?;
    publish(connection, audit)?;
    Ok(record.id)
}
