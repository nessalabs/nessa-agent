//! One transaction advances the contiguous source cursor with its delivery.
use super::audit::Record;
use super::journal::{commit, delivery_audit, invalidate_source};
use super::publication::{append, publish};
use super::stored::{self, StoredDelivery};
use super::{codec, cursor, integer, unavailable};
use crate::webhooks::application::{DeliveryCause, OutboxError, QueuedRecord, MAX_BODY_BYTES};
use crate::webhooks::domain::Delivery;
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension};
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
    cursor::check_identity(&registration, &conversation, &record.scope)?;
    let scope = codec::scope_text(&record.scope)?;
    let position = integer(record.position)?;
    let transaction = connection.transaction().map_err(unavailable)?;
    let current_registration = transaction
        .query_row(
            &format!("{} FROM registrations WHERE id=?1", super::registry::SELECT),
            [&registration],
            |row| super::registry::StoredRegistration::row(row, 0),
        )
        .optional()
        .map_err(super::cursor::row_error)?
        .map(super::registry::StoredRegistration::checked)
        .transpose()?;
    if !current_registration.is_some_and(|saved| saved.lifecycle.retains_queued()) {
        return Err(OutboxError::Conflict);
    }
    let current = cursor::read(&transaction, record.registration, &record.conversation)?;
    let existing:Option<(StoredDelivery,Vec<u8>)>=transaction.query_row(
        &format!("{},body FROM deliveries WHERE registration=?1 AND conversation=?2 AND incarnation=?3 AND position=?4",stored::SELECT),
        params![registration,conversation,record.scope.incarnation().as_str(),position],
        |r|Ok((StoredDelivery::row(r)?,r.get(10)?)),
    ).optional().map_err(unavailable)?;
    if let Some((saved, body)) = existing {
        let existing = saved.checked(&transaction)?;
        if codec::scope_text(&existing.scope)? != scope
            || body != record.body
            || existing.delivery.epoch() != record.epoch
        {
            return Err(OutboxError::Conflict);
        }
        return Ok(existing.delivery.id());
    }
    if let Some(current) = &current {
        let old = &current.scope;
        if current.reset_required {
            return Err(OutboxError::ResetRequired);
        }
        if !codec::same_source(old, &record.scope) {
            let fact = super::source_reset::Fact {
                id: Uuid::new_v4(),
                registration: record.registration,
                conversation: record.conversation.clone(),
                prior_scope: old.clone(),
                prior_position: current.position,
                observed_scope: record.scope.clone(),
                stop: codec::SourceStop {
                    initiator: record.initiator,
                    at_ms: record.observed_at_ms,
                },
            };
            super::source_reset::insert(&transaction, &fact)?;
            let changed=transaction.execute("UPDATE cursors SET reset_required=1,source_reset=?3 WHERE registration=?1 AND conversation=?2 AND reset_required=0",params![registration,conversation,fact.id.to_string()]).map_err(unavailable)?;
            if changed != 1 {
                return Err(OutboxError::Conflict);
            }
            invalidate_source(&transaction, &fact)?;
            append(&transaction, Record::cursor(&fact))?;
            commit(
                transaction,
                Some(record.registration),
                record.observed_at_ms,
            )?;
            publish(connection, audit)?;
            return Err(OutboxError::ResetRequired);
        }
        if current.position != record.expected_after {
            return Err(OutboxError::Conflict);
        }
    } else if record.expected_after != 0 {
        return Err(OutboxError::Conflict);
    }
    if current.is_none() {
        super::capacity::admit_cursor(&transaction, record.registration)?;
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
