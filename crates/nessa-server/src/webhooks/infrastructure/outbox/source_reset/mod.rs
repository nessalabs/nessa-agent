//! One immutable reset owns original source provenance and capture membership.
mod fact;
mod retention;
use super::{codec, integer, invalid, unavailable, uuid};
use crate::webhooks::{
    application::{OutboxError, SOURCE_RESET_BYTES},
    domain::{Delivery, Phase},
};
pub(super) use fact::Fact;
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension, Transaction};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
pub(super) use retention::{validate_all, validate_owed, validate_schema};
use uuid::Uuid;

pub(super) const READ: &str = "SELECT registration,CASE WHEN bytes<=?2 THEN fact ELSE NULL END,bytes FROM source_reset_facts WHERE id=?1";

pub(super) struct Capture {
    pub fact: Fact,
    pub before: Delivery,
}

pub(super) fn read(connection: &Connection, id: Uuid) -> Result<Option<Fact>, OutboxError> {
    let saved: Option<(String, Option<String>, i64)> = connection
        .query_row(
            READ,
            params![id.to_string(), SOURCE_RESET_BYTES as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(super::cursor::row_error)?;
    saved
        .map(|(registration, text, bytes)| {
            let text = text.ok_or(OutboxError::Invalid)?;
            let fact = Fact::decode(&text)?;
            if fact.id != id
                || fact.registration != uuid(&registration)?
                || bytes != text.len() as i64
            {
                return Err(OutboxError::Invalid);
            }
            Ok(fact)
        })
        .transpose()
}

pub(super) fn insert(transaction: &Transaction<'_>, fact: &Fact) -> Result<(), OutboxError> {
    retention::check_capacity(transaction, &fact.registration.to_string())?;
    transaction
        .execute(
            "INSERT INTO source_reset_facts(id,registration,fact) VALUES(?1,?2,?3)",
            params![
                fact.id.to_string(),
                fact.registration.to_string(),
                fact.encode()?
            ],
        )
        .map_err(unavailable)?;
    Ok(())
}

pub(super) fn capture(
    transaction: &Transaction<'_>,
    fact: &Fact,
    saved: &super::stored::CheckedDelivery,
) -> Result<(), OutboxError> {
    compatible(
        fact,
        saved.registration,
        &saved.conversation,
        &saved.scope,
        saved.position,
        &saved.delivery,
    )?;
    if !matches!(
        saved.delivery.phase(),
        Phase::Queued | Phase::Sending | Phase::Retry { .. }
    ) {
        return Err(OutboxError::Invalid);
    }
    transaction
        .execute(
            "INSERT INTO source_reset_members(delivery,reset,position,state) VALUES(?1,?2,?3,?4)",
            params![
                saved.delivery.id().to_string(),
                fact.id.to_string(),
                integer(saved.position)?,
                codec::encode(&saved.delivery)?
            ],
        )
        .map_err(|_| OutboxError::Conflict)?;
    Ok(())
}

fn compatible(
    fact: &Fact,
    registration: Uuid,
    conversation: &ConversationId,
    scope: &Scope,
    position: u64,
    delivery: &Delivery,
) -> Result<(), OutboxError> {
    if fact.registration != registration
        || &fact.conversation != conversation
        || !codec::same_source(&fact.prior_scope, scope)
        || position == 0
        || position > fact.prior_position
        || scope.access_epoch().as_str() != format!("epoch-{}", delivery.epoch())
    {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}

pub(super) fn delivery(
    connection: &Connection,
    registration: Uuid,
    conversation: &ConversationId,
    scope: &Scope,
    position: u64,
    current: &Delivery,
) -> Result<Option<Capture>, OutboxError> {
    let saved:Option<(String,i64,Option<String>)>=connection.query_row("SELECT reset,position,CASE WHEN length(CAST(state AS BLOB))<=?2 THEN state ELSE NULL END FROM source_reset_members WHERE delivery=?1",params![current.id().to_string(),codec::STATE_BYTES as i64],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(super::cursor::row_error)?;
    saved
        .map(|(id, prior_position, state)| {
            let fact = read(connection, uuid(&id)?)?.ok_or(OutboxError::Invalid)?;
            compatible(&fact, registration, conversation, scope, position, current)?;
            let before = codec::decode(&state.ok_or(OutboxError::Invalid)?)?;
            if before.id() != current.id()
                || before.epoch() != current.epoch()
                || u64::try_from(prior_position).map_err(invalid)? != position
            {
                return Err(OutboxError::Invalid);
            }
            match before.phase() {
                Phase::Queued | Phase::Retry { .. } if before.refused().as_ref() == Ok(current) => {
                }
                Phase::Sending
                    if before.attempts() == current.attempts()
                        && before.timestamp_ms() == current.timestamp_ms()
                        && current.phase() != Phase::Queued => {}
                _ => return Err(OutboxError::Invalid),
            }
            Ok(Capture { fact, before })
        })
        .transpose()
}
