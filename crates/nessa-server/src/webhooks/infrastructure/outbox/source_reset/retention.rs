//! Indexed reference accounting and restoration of its canonical schema.
use super::{
    super::{invalid, unavailable, uuid},
    read,
};
use crate::webhooks::application::{
    OutboxError, MAX_REGISTRATIONS, MAX_SOURCE_RESETS, PENDING_LIMIT, RETAINED_LIMIT,
    SOURCE_RESET_BYTES,
};
use nessa_local_database::rusqlite::{Connection, OptionalExtension};

pub(in super::super) fn check_capacity(
    connection: &Connection,
    registration: &str,
) -> Result<(), OutboxError> {
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM source_reset_facts WHERE registration=?1",
            [registration],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    if count >= MAX_SOURCE_RESETS as i64 {
        return Err(OutboxError::Capacity);
    }
    Ok(())
}

fn check_references(connection: &Connection, id: &str, refs: i64) -> Result<(), OutboxError> {
    let actual:i64=connection.query_row("SELECT (SELECT count(*) FROM cursors WHERE source_reset=?1)+(SELECT count(*) FROM source_reset_members WHERE reset=?1)+(SELECT count(*) FROM audit WHERE source_reset=?1)",[id],|r|r.get(0)).map_err(unavailable)?;
    if refs <= 0 || actual != refs {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}

pub(in super::super) fn validate_all(connection: &Connection) -> Result<(), OutboxError> {
    if connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(unavailable)?
        .exists([])
        .map_err(unavailable)?
    {
        return Err(OutboxError::Invalid);
    }
    for (table, limit) in [
        ("source_reset_facts", MAX_REGISTRATIONS * MAX_SOURCE_RESETS),
        (
            "source_reset_members",
            MAX_REGISTRATIONS * (PENDING_LIMIT + RETAINED_LIMIT),
        ),
    ] {
        let sql = format!("SELECT count(*) FROM {table}");
        let count: i64 = connection
            .query_row(&sql, [], |r| r.get(0))
            .map_err(unavailable)?;
        if count > limit as i64 {
            return Err(OutboxError::Invalid);
        }
    }
    if connection
        .prepare(
            "SELECT 1 FROM source_reset_facts GROUP BY registration HAVING count(*)>?1 LIMIT 1",
        )
        .map_err(unavailable)?
        .exists([MAX_SOURCE_RESETS as i64])
        .map_err(unavailable)?
    {
        return Err(OutboxError::Invalid);
    }
    let mut q = connection
        .prepare("SELECT id,bytes,refs FROM source_reset_facts")
        .map_err(unavailable)?;
    let mut rows = q.query([]).map_err(unavailable)?;
    while let Some(row) = rows.next().map_err(unavailable)? {
        let id: String = row.get(0).map_err(invalid)?;
        let bytes: i64 = row.get(1).map_err(invalid)?;
        let refs: i64 = row.get(2).map_err(invalid)?;
        if bytes < 0 || bytes > SOURCE_RESET_BYTES as i64 {
            return Err(OutboxError::Invalid);
        }
        let fact = read(connection, uuid(&id)?)?.ok_or(OutboxError::Invalid)?;
        super::owner::verify_receipt(connection, &fact)?;
        check_references(connection, &id, refs)?;
    }
    Ok(())
}

pub(in super::super) fn validate_owed(connection: &Connection) -> Result<(), OutboxError> {
    let mut q=connection.prepare("SELECT DISTINCT source_reset FROM audit WHERE published=0 AND source_reset IS NOT NULL").map_err(unavailable)?;
    let mut rows = q.query([]).map_err(unavailable)?;
    while let Some(row) = rows.next().map_err(unavailable)? {
        let id: String = row.get(0).map_err(invalid)?;
        let refs: i64 = connection
            .query_row(
                "SELECT refs FROM source_reset_facts WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()
            .map_err(super::super::cursor::row_error)?
            .ok_or(OutboxError::Invalid)?;
        let fact = read(connection, uuid(&id)?)?.ok_or(OutboxError::Invalid)?;
        super::owner::verify_receipt(connection, &fact)?;
        check_references(connection, &id, refs)?;
    }
    Ok(())
}
