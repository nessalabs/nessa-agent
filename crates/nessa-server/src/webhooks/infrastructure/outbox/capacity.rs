//! One accounting check serves restored files and each state commit.
use super::unavailable;
use crate::webhooks::application::{
    OutboxError, MAX_CURSORS, MAX_REGISTRATIONS, PENDING_LIMIT, RETAINED_LIMIT,
};
use nessa_local_database::rusqlite::{params, Connection};
use uuid::Uuid;

pub(super) fn check(
    connection: &Connection,
    registration: Option<Uuid>,
) -> Result<(), OutboxError> {
    let count: i64 = connection
        .query_row("SELECT count(*) FROM registrations", [], |r| r.get(0))
        .map_err(unavailable)?;
    if count > MAX_REGISTRATIONS as i64 {
        return Err(OutboxError::Capacity);
    }
    let selection = if registration.is_some() {
        "registration=?1"
    } else {
        "?1 IS NULL"
    };
    let sql = format!(
        "SELECT registration FROM deliveries WHERE {selection} GROUP BY registration
        HAVING sum(phase IN ('queued','sending','retry'))>?2
            OR sum(phase NOT IN ('queued','sending','retry'))>?3 LIMIT 1"
    );
    if connection
        .prepare(&sql)
        .map_err(unavailable)?
        .exists(params![
            registration.map(|id| id.to_string()),
            PENDING_LIMIT as i64,
            RETAINED_LIMIT as i64
        ])
        .map_err(unavailable)?
    {
        return Err(OutboxError::Capacity);
    }
    let cursor_sql = format!(
        "SELECT 1 FROM cursors WHERE {selection} GROUP BY registration HAVING count(*)>?2 LIMIT 1"
    );
    if connection
        .prepare(&cursor_sql)
        .map_err(unavailable)?
        .exists(params![
            registration.map(|id| id.to_string()),
            MAX_CURSORS as i64
        ])
        .map_err(unavailable)?
    {
        return Err(OutboxError::Capacity);
    }
    Ok(())
}

/// Reserve a new cursor before its delivery and audit are inserted.
pub(super) fn admit_cursor(connection: &Connection, registration: Uuid) -> Result<(), OutboxError> {
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM cursors WHERE registration=?1",
            [registration.to_string()],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    if count >= MAX_CURSORS as i64 {
        return Err(OutboxError::Capacity);
    }
    Ok(())
}
