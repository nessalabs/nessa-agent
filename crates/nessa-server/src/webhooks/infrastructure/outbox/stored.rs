//! One decoder correlates durable delivery identity with its enclosing SQL row.
use super::{capacity, codec, cursor, invalid, unavailable, uuid};
use crate::webhooks::application::OutboxError;
use crate::webhooks::domain::{Delivery, Phase};
use nessa_local_database::rusqlite::{Connection, OptionalExtension, Result as SqlResult, Row};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
use uuid::Uuid;

pub(super) const SELECT: &str =
    "SELECT id,registration,conversation,incarnation,position,scope,state,phase,due_at,(SELECT reset FROM source_reset_members WHERE delivery=deliveries.id)";

pub(super) struct StoredDelivery {
    id: String,
    registration: String,
    conversation: String,
    incarnation: String,
    position: i64,
    scope: String,
    state: String,
    phase: String,
    due_at: i64,
    source_reset: Option<String>,
}

pub(super) struct CheckedDelivery {
    pub registration: Uuid,
    pub conversation: ConversationId,
    pub position: u64,
    pub scope: Scope,
    pub delivery: Delivery,
    pub saved_state: String,
    pub source_stop: Option<codec::SourceStop>,
    pub source_reset: Option<Uuid>,
    pub capture_before: Option<Delivery>,
}

impl StoredDelivery {
    /// The first ten columns follow SELECT; a joined row may append payload
    /// and registration facts after those columns.
    pub fn row(row: &Row<'_>) -> SqlResult<Self> {
        Ok(Self {
            id: row.get(0)?,
            registration: row.get(1)?,
            conversation: row.get(2)?,
            incarnation: row.get(3)?,
            position: row.get(4)?,
            scope: row.get(5)?,
            state: row.get(6)?,
            phase: row.get(7)?,
            due_at: row.get(8)?,
            source_reset: row.get(9)?,
        })
    }
    pub fn checked(self, connection: &Connection) -> Result<CheckedDelivery, OutboxError> {
        let delivery = codec::decode(&self.state)?;
        let scope = codec::scope(&self.scope)?;
        let registration = uuid(&self.registration)?;
        let conversation = ConversationId::new(&self.conversation).map_err(invalid)?;
        let position = u64::try_from(self.position).map_err(invalid)?;
        let due = u64::try_from(self.due_at).map_err(invalid)?;
        if delivery.id() != uuid(&self.id)?
            || codec::phase(&delivery) != (self.phase.as_str(), due)
            || position == 0
            || scope.receiver().as_str() != registration.to_string()
            || scope.stream().as_str() != conversation.to_string()
            || scope.incarnation().as_str() != self.incarnation
            || scope.access_epoch().as_str() != format!("epoch-{}", delivery.epoch())
        {
            return Err(OutboxError::Invalid);
        }
        let captured = super::source_reset::delivery(
            connection,
            registration,
            &conversation,
            &scope,
            position,
            &delivery,
        )?;
        let source_reset = captured.as_ref().map(|c| c.fact.id);
        if self.source_reset.as_ref().map(|id| uuid(id)).transpose()? != source_reset {
            return Err(OutboxError::Invalid);
        }
        let source_stop = captured.as_ref().map(|c| c.fact.stop);
        let capture_before = captured.map(|c| c.before);
        Ok(CheckedDelivery {
            registration,
            conversation,
            position,
            scope,
            delivery,
            saved_state: self.state,
            source_stop,
            source_reset,
            capture_before,
        })
    }
}

pub(super) fn by_id(
    connection: &Connection,
    id: Uuid,
) -> Result<Option<CheckedDelivery>, OutboxError> {
    connection
        .query_row(
            &format!("{SELECT} FROM deliveries WHERE id=?1"),
            [id.to_string()],
            StoredDelivery::row,
        )
        .optional()
        .map_err(unavailable)?
        .map(|saved| saved.checked(connection))
        .transpose()
}

/// Startup precedes any filtered scan, audit publication or retirement. The
/// lifetime owner lock then excludes another store writer; generated phase/due
/// columns and checked mutations preserve these relationships.
pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    let projections = {
        let mut statement=connection.prepare("SELECT name,hidden FROM pragma_table_xinfo('deliveries') WHERE name IN ('phase','due_at') ORDER BY name").map_err(unavailable)?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(unavailable)?;
        rows.collect::<SqlResult<Vec<_>>>().map_err(unavailable)?
    };
    if projections != [("due_at".into(), 3), ("phase".into(), 3)] {
        return Err(OutboxError::Invalid);
    }
    capacity::check(connection, None).map_err(|error| {
        if error == OutboxError::Capacity {
            OutboxError::Invalid
        } else {
            error
        }
    })?;
    super::registry::validate(connection)?;
    let mut statement = connection
        .prepare(&format!("{SELECT} FROM deliveries"))
        .map_err(unavailable)?;
    let rows = statement
        .query_map([], StoredDelivery::row)
        .map_err(unavailable)?;
    for row in rows {
        let saved = row.map_err(unavailable)?.checked(connection)?;
        check_current(connection, &saved)?;
    }
    cursor::validate(connection)?;
    if connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(unavailable)?
        .exists([])
        .map_err(unavailable)?
    {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}

pub(super) fn check_current(
    connection: &Connection,
    saved: &CheckedDelivery,
) -> Result<(), OutboxError> {
    if matches!(saved.delivery.phase(), Phase::Terminal(_)) || saved.source_stop.is_some() {
        return Ok(());
    }
    let current = cursor::read(connection, saved.registration, &saved.conversation)?
        .ok_or(OutboxError::Invalid)?;
    if current.reset_required
        || current.position < saved.position
        || !codec::same_source(&current.scope, &saved.scope)
    {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}
