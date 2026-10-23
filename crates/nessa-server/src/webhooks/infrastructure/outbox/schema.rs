//! The one current private outbox schema.
use super::{codec::check_row_bytes, cursor::row_error, unavailable};
use crate::webhooks::application::OutboxError;
use nessa_local_database::rusqlite::{Connection, Result as SqlResult, Row};

pub(super) const SCHEMA: &str = include_str!("schema.sql");

#[derive(PartialEq, Eq)]
struct SchemaEntry {
    kind: String,
    name: String,
    table: String,
    sql: Option<String>,
}
impl SchemaEntry {
    fn row(row: &Row<'_>) -> SqlResult<Self> {
        check_row_bytes(row, 4, 2 * SCHEMA.len())?;
        Ok(Self {
            kind: row.get(0)?,
            name: row.get(1)?,
            table: row.get(2)?,
            sql: row.get(3)?,
        })
    }
}

fn definitions(connection: &Connection, limit: i64) -> Result<Vec<SchemaEntry>, OutboxError> {
    let mut query=connection.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type,name LIMIT ?1").map_err(unavailable)?;
    let rows = query
        .query_map([limit], SchemaEntry::row)
        .map_err(unavailable)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(row_error)
}

/// SQLite's normalized definitions enforce the one current owned schema,
/// including immutable facts, intent references and transactional cleanup.
pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    let expected = Connection::open_in_memory().map_err(unavailable)?;
    expected.execute_batch(SCHEMA).map_err(unavailable)?;
    let expected = definitions(&expected, -1)?;
    if definitions(connection, expected.len() as i64 + 1)? != expected {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/webhooks/infrastructure/outbox/schema.rs"]
mod tests;
