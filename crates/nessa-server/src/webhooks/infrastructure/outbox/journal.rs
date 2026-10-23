//! State/audit commits, immutable journal publication and bounded retirement.
use super::audit::{Record, RegistrationChange};
use super::stored::{self, StoredDelivery};
use super::{capacity, codec, invalid, unavailable, uuid};
use crate::webhooks::application::{
    DeliveryCause, OutboxError, MAX_REGISTRATIONS, PENDING_LIMIT, RETAINED_LIMIT,
};
use crate::webhooks::domain::{Delivery, Phase};
use nessa_local_database::rusqlite::{params, Connection, Result as SqlResult, Transaction};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
use std::path::Path;
use uuid::Uuid;

use super::publication::{append, publish};

pub(super) fn refuse_pending(
    transaction: &Transaction<'_>,
    registration: Uuid,
    conversation: Option<&ConversationId>,
    cause: DeliveryCause,
    at_ms: u64,
) -> Result<(), OutboxError> {
    let states = {
        let mut statement=transaction.prepare(&format!("{} FROM deliveries WHERE registration=?1 AND (?2 IS NULL OR conversation=?2) AND phase IN ('queued','retry') ORDER BY rowid LIMIT ?3",stored::SELECT)).map_err(unavailable)?;
        let rows = statement
            .query_map(
                params![
                    registration.to_string(),
                    conversation.map(ToString::to_string),
                    (PENDING_LIMIT + 1) as i64
                ],
                StoredDelivery::row,
            )
            .map_err(unavailable)?;
        rows.collect::<SqlResult<Vec<_>>>().map_err(unavailable)?
    };
    if states.len() > PENDING_LIMIT {
        return Err(OutboxError::Invalid);
    }
    for saved in states {
        let saved = saved.checked()?;
        if saved.registration != registration
            || conversation.is_some_and(|id| *id != saved.conversation)
        {
            return Err(OutboxError::Invalid);
        }
        refuse_one(transaction, saved, cause, at_ms)?;
    }
    Ok(())
}

pub(super) fn delivery_audit(
    transaction: &Transaction<'_>,
    after: &Delivery,
    before: Option<&Delivery>,
    cause: DeliveryCause,
    at_ms: u64,
) -> Result<(), OutboxError> {
    let saved = stored::by_id(transaction, after.id())?.ok_or(OutboxError::Conflict)?;
    if saved.delivery != *after {
        return Err(OutboxError::Invalid);
    }
    if let Some(before) = before {
        if !cause.allows(before, after, at_ms) {
            return Err(OutboxError::Invalid);
        }
    } else if !matches!(cause, DeliveryCause::SourceQueued { .. }) {
        return Err(OutboxError::Invalid);
    }
    let registration_facts = super::registry::by_id(transaction, saved.registration)?;
    let record = Record::delivery(
        &saved,
        &registration_facts.registration.target,
        before,
        cause,
        registration_facts.stop.as_ref(),
        at_ms,
    )?;
    append(transaction, record)
}

/// Retention ranks all terminal rows, including this transaction's newly owed
/// audit, while deletion remains conditional on the old row's published audit.
fn trim(connection: &Connection) -> Result<(), OutboxError> {
    connection
        .execute(
            "DELETE FROM deliveries WHERE id IN (
            SELECT id FROM (
                SELECT d.id,row_number() OVER (
                    PARTITION BY registration ORDER BY d.rowid DESC
                ) AS retained FROM deliveries d
                WHERE phase NOT IN ('queued','sending','retry')
            ) WHERE retained>?1
        ) AND NOT EXISTS (
            SELECT 1 FROM audit a WHERE a.delivery=deliveries.id AND a.published=0
        )",
            [RETAINED_LIMIT as i64],
        )
        .map_err(unavailable)?;
    connection.execute("DELETE FROM deliveries WHERE registration IN (SELECT id FROM registrations WHERE active=0) AND phase NOT IN ('queued','sending','retry') AND NOT EXISTS(SELECT 1 FROM audit a WHERE a.delivery=deliveries.id AND a.published=0)",[]).map_err(unavailable)?;
    connection.execute("DELETE FROM audit WHERE published=1 AND (delivery IS NULL OR NOT EXISTS(SELECT 1 FROM deliveries d WHERE d.id=audit.delivery))",[]).map_err(unavailable)?;
    Ok(())
}

/// Publish first; only then retire stopped projections and their private
/// registration material. Retire evidence commits with the deletes, and any
/// failed projection is replayed before the next operation by `run`.
pub(super) fn finish(
    connection: &mut Connection,
    audit: &Path,
    at_ms: u64,
) -> Result<(), OutboxError> {
    publish(connection, audit)?;
    let transaction = connection.transaction().map_err(unavailable)?;
    let stopped = {
        let mut statement = transaction
            .prepare("SELECT id FROM registrations WHERE active=0 LIMIT ?1")
            .map_err(unavailable)?;
        let rows = statement
            .query_map([(MAX_REGISTRATIONS + 1) as i64], |r| r.get::<_, String>(0))
            .map_err(unavailable)?;
        rows.collect::<SqlResult<Vec<_>>>().map_err(unavailable)?
    };
    if stopped.len() > MAX_REGISTRATIONS {
        return Err(OutboxError::Invalid);
    }
    for id in stopped {
        refuse_pending(
            &transaction,
            uuid(&id)?,
            None,
            DeliveryCause::AuthorityStopped,
            at_ms,
        )?;
    }
    refuse_invalidated(&transaction, at_ms)?;
    commit(transaction, None, at_ms)?;
    publish(connection, audit)
}

fn retire_drained(transaction: &Transaction<'_>, at_ms: u64) -> Result<(), OutboxError> {
    let retired = {
        let mut statement=transaction.prepare("SELECT id FROM registrations r WHERE active=0 AND NOT EXISTS(SELECT 1 FROM deliveries d WHERE d.registration=r.id) LIMIT ?1").map_err(unavailable)?;
        let rows = statement
            .query_map([(MAX_REGISTRATIONS + 1) as i64], |r| r.get::<_, String>(0))
            .map_err(unavailable)?;
        rows.collect::<SqlResult<Vec<_>>>().map_err(unavailable)?
    };
    if retired.len() > MAX_REGISTRATIONS {
        return Err(OutboxError::Invalid);
    }
    for id in retired {
        let saved = super::registry::by_id(transaction, uuid(&id)?)?;
        let stop = saved.stop.as_ref().ok_or(OutboxError::Invalid)?;
        append(
            transaction,
            Record::registration(&saved.registration, RegistrationChange::Retire(stop), at_ms),
        )?;
        transaction
            .execute("DELETE FROM cursors WHERE registration=?1", [&id])
            .map_err(unavailable)?;
        transaction
            .execute("DELETE FROM registrations WHERE id=?1", [&id])
            .map_err(unavailable)?;
    }
    Ok(())
}

pub(super) fn commit(
    transaction: Transaction<'_>,
    registration: Option<Uuid>,
    at_ms: u64,
) -> Result<(), OutboxError> {
    trim(&transaction)?;
    retire_drained(&transaction, at_ms)?;
    capacity::check(&transaction, registration)?;
    transaction.commit().map_err(unavailable)
}

/// The cursor keeps the new refusal, and admitted rows retain its causal fact
/// across a later cursor reset. Sending itself still resolves its observation.
pub(super) fn invalidate_source(
    transaction: &Transaction<'_>,
    registration: Uuid,
    conversation: &ConversationId,
    old: &Scope,
    stop: codec::SourceStop,
) -> Result<(), OutboxError> {
    let rows = {
        let mut statement=transaction.prepare(&format!("{} FROM deliveries WHERE registration=?1 AND conversation=?2 AND phase IN ('queued','retry','sending') LIMIT ?3",stored::SELECT)).map_err(unavailable)?;
        let rows = statement
            .query_map(
                params![
                    registration.to_string(),
                    conversation.to_string(),
                    (PENDING_LIMIT + 1) as i64
                ],
                StoredDelivery::row,
            )
            .map_err(unavailable)?;
        rows.collect::<SqlResult<Vec<_>>>().map_err(unavailable)?
    };
    if rows.len() > PENDING_LIMIT {
        return Err(OutboxError::Invalid);
    }
    for row in rows {
        let saved = row.checked()?;
        if !codec::same_source(&saved.scope, old) {
            continue;
        }
        if saved.delivery.phase() == Phase::Sending {
            if saved.source_stop.is_none() {
                let changed=transaction.execute("UPDATE deliveries SET source_stop=?1 WHERE id=?2 AND state=?3 AND source_stop IS NULL",params![codec::source_stop_text(stop)?,saved.delivery.id().to_string(),saved.saved_state]).map_err(unavailable)?;
                if changed != 1 {
                    return Err(OutboxError::Conflict);
                }
            }
        } else {
            refuse_one(
                transaction,
                saved,
                DeliveryCause::SourceChanged {
                    initiator: stop.initiator,
                },
                stop.at_ms,
            )?;
        }
    }
    Ok(())
}

fn refuse_one(
    transaction: &Transaction<'_>,
    saved: stored::CheckedDelivery,
    cause: DeliveryCause,
    at_ms: u64,
) -> Result<(), OutboxError> {
    let before = saved.delivery;
    let after = before.refused().map_err(invalid)?;
    let changed = transaction
        .execute(
            "UPDATE deliveries SET state=?1 WHERE id=?2 AND state=?3",
            params![
                codec::encode(&after)?,
                after.id().to_string(),
                saved.saved_state
            ],
        )
        .map_err(unavailable)?;
    if changed != 1 {
        return Err(OutboxError::Conflict);
    }
    delivery_audit(transaction, &after, Some(&before), cause, at_ms)
}

fn refuse_invalidated(transaction: &Transaction<'_>, at_ms: u64) -> Result<(), OutboxError> {
    let registrations = {
        let mut statement = transaction
            .prepare("SELECT id FROM registrations LIMIT ?1")
            .map_err(unavailable)?;
        let rows = statement
            .query_map([(MAX_REGISTRATIONS + 1) as i64], |r| r.get::<_, String>(0))
            .map_err(unavailable)?;
        rows.collect::<SqlResult<Vec<_>>>().map_err(unavailable)?
    };
    if registrations.len() > MAX_REGISTRATIONS {
        return Err(OutboxError::Invalid);
    }
    for registration in registrations {
        let rows = {
            let mut statement=transaction.prepare(&format!("{} FROM deliveries WHERE registration=?1 AND source_stop IS NOT NULL AND phase='retry' LIMIT ?2",stored::SELECT)).map_err(unavailable)?;
            let rows = statement
                .query_map(
                    params![registration, (PENDING_LIMIT + 1) as i64],
                    StoredDelivery::row,
                )
                .map_err(unavailable)?;
            rows.collect::<SqlResult<Vec<_>>>().map_err(unavailable)?
        };
        if rows.len() > PENDING_LIMIT {
            return Err(OutboxError::Invalid);
        }
        for row in rows {
            let saved = row.checked()?;
            let stop = saved.source_stop.ok_or(OutboxError::Invalid)?;
            refuse_one(
                transaction,
                saved,
                DeliveryCause::SourceChanged {
                    initiator: stop.initiator,
                },
                at_ms,
            )?;
        }
    }
    Ok(())
}
