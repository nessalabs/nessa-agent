//! One decoder correlates durable delivery identity with its enclosing SQL row.
use super::{capacity, codec, invalid, unavailable, uuid};
use crate::webhooks::application::OutboxError;
use crate::webhooks::domain::{Delivery, Phase};
use nessa_local_database::rusqlite::{Connection, OptionalExtension, Result as SqlResult, Row};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
use uuid::Uuid;

pub(super) const SELECT: &str =
    "SELECT id,registration,conversation,incarnation,position,scope,state,phase,due_at,source_stop";

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
    source_stop: Option<String>,
}

pub(super) struct CheckedDelivery {
    pub registration: Uuid,
    pub conversation: ConversationId,
    pub position: u64,
    pub scope: Scope,
    pub delivery: Delivery,
    pub saved_state: String,
    pub source_stop: Option<codec::SourceStop>,
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
            source_stop: row.get(9)?,
        })
    }
    pub fn checked(self) -> Result<CheckedDelivery, OutboxError> {
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
        let source_stop = self
            .source_stop
            .as_deref()
            .map(codec::source_stop)
            .transpose()?;
        if source_stop.is_some() && delivery.phase() == Phase::Queued {
            return Err(OutboxError::Invalid);
        }
        Ok(CheckedDelivery {
            registration,
            conversation,
            position,
            scope,
            delivery,
            saved_state: self.state,
            source_stop,
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
        .map(StoredDelivery::checked)
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
    let mut statement = connection
        .prepare(&format!("{SELECT} FROM deliveries"))
        .map_err(unavailable)?;
    let rows = statement
        .query_map([], StoredDelivery::row)
        .map_err(unavailable)?;
    for row in rows {
        let saved = row.map_err(unavailable)?.checked()?;
        check_current(connection, &saved)?;
    }
    let mut statement = connection
        .prepare("SELECT registration,conversation,scope FROM cursors")
        .map_err(unavailable)?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(unavailable)?;
    for row in rows {
        let (registration, conversation, scope) = row.map_err(unavailable)?;
        check_cursor(&registration, &conversation, &codec::scope(&scope)?)?;
    }
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

pub(super) fn check_cursor(
    registration: &str,
    conversation: &str,
    scope: &Scope,
) -> Result<(), OutboxError> {
    let registration = uuid(registration)?;
    let conversation = ConversationId::new(conversation).map_err(invalid)?;
    if scope.receiver().as_str() != registration.to_string()
        || scope.stream().as_str() != conversation.to_string()
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
    let cursor:Option<(String,i64,i64)>=connection.query_row("SELECT scope,position,reset_required FROM cursors WHERE registration=?1 AND conversation=?2",[saved.registration.to_string(),saved.conversation.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(unavailable)?;
    let Some((scope, position, reset)) = cursor else {
        return Err(OutboxError::Invalid);
    };
    let scope = codec::scope(&scope)?;
    check_cursor(
        &saved.registration.to_string(),
        &saved.conversation.to_string(),
        &scope,
    )?;
    if reset != 0 || position < saved.position as i64 || !codec::same_source(&scope, &saved.scope) {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}
