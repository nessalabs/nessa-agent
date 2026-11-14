//! Statements kept prepared on the store's one connection.
//!
//! `Connection::query_row` and `Connection::execute` compile their SQL every
//! call. A list resolves several statements per conversation it names, so a
//! thousand-row list compiled thousands of statements and spent its time in
//! SQLite's parser. These keep each statement compiled, keyed by its text, in
//! the connection's own cache (sized in `LocalConversationStore::open`).
use nessa_local_database::rusqlite::{Connection, Params, Result, Row};

/// The store's statements, run through the connection's statement cache.
pub(super) trait CachedSql {
    fn cached_row<T, P: Params, F: FnOnce(&Row<'_>) -> Result<T>>(
        &self,
        sql: &str,
        params: P,
        row: F,
    ) -> Result<T>;
    fn cached_execute<P: Params>(&self, sql: &str, params: P) -> Result<usize>;
}

impl CachedSql for Connection {
    fn cached_row<T, P: Params, F: FnOnce(&Row<'_>) -> Result<T>>(
        &self,
        sql: &str,
        params: P,
        row: F,
    ) -> Result<T> {
        self.prepare_cached(sql)?.query_row(params, row)
    }
    fn cached_execute<P: Params>(&self, sql: &str, params: P) -> Result<usize> {
        self.prepare_cached(sql)?.execute(params)
    }
}

/// Distinct statements the store runs, with room to spare: past this the
/// cache drops the least recently used, which costs a compile, not an answer.
pub(super) const STATEMENTS: usize = 256;
