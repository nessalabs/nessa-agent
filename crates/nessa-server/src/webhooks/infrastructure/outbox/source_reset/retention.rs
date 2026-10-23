//! Indexed reference accounting and restoration of its canonical schema.
use super::{
    super::{invalid, unavailable, uuid, SCHEMA},
    read,
};
use crate::webhooks::application::{
    OutboxError, MAX_REGISTRATIONS, MAX_SOURCE_RESETS, PENDING_LIMIT, RETAINED_LIMIT,
    SOURCE_RESET_BYTES,
};
use nessa_local_database::rusqlite::{Connection, OptionalExtension};

#[derive(PartialEq, Eq)]
struct SchemaEntry {
    kind: String,
    name: String,
    table: String,
    sql: Option<String>,
}

pub(in super::super) fn validate_schema(connection: &Connection) -> Result<(), OutboxError> {
    // Compare against SQLite's own normalized representation of the ONE schema,
    // including immutable-member/fact guards and reference accounting triggers.
    let expected = Connection::open_in_memory().map_err(unavailable)?;
    expected.execute_batch(SCHEMA).map_err(unavailable)?;
    let definitions = |c: &Connection, limit: i64| -> Result<Vec<SchemaEntry>, OutboxError> {
        let mut q=c.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name GLOB 'source_reset_*' OR name IN ('cursors','deliveries','audit') ORDER BY type,name LIMIT ?1").map_err(unavailable)?;
        let rows = q
            .query_map([limit], |r| {
                Ok(SchemaEntry {
                    kind: r.get(0)?,
                    name: r.get(1)?,
                    table: r.get(2)?,
                    sql: r.get(3)?,
                })
            })
            .map_err(unavailable)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(unavailable)
    };
    let expected = definitions(&expected, -1)?;
    if definitions(connection, expected.len() as i64 + 1)? != expected {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}

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
        read(connection, uuid(&id)?)?.ok_or(OutboxError::Invalid)?;
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
        read(connection, uuid(&id)?)?.ok_or(OutboxError::Invalid)?;
        check_references(connection, &id, refs)?;
    }
    Ok(())
}
