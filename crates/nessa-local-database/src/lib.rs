//! One private SQLite database for one context, opened at the schema version
//! that context was built with and no other.
//!
//! A context that needs to ask its data more than "give me this identity" —
//! whose these are, newest first, which are unfinished — keeps it here rather
//! than in one file per identity. The context owns its schema and its tables;
//! this crate owns only how the file is opened:
//!
//! ```text
//!   composition ── path, Schema ──▶ open ──▶ nessa-local-storage (private dir, private file)
//!                                     │
//!                                     └──▶ SQLite: pragmas, then the schema's version
//! ```
//!
//! Arrows are calls. The directory must already be private and this OS
//! user's; the file is created private when absent, before SQLite ever sees
//! it (`the_file_is_created_private_before_sqlite_opens_it`).
//!
//! Every connection has foreign keys enforced, a rollback journal synced in
//! full (through the drive's cache on macOS), and `secure_delete`, so a deleted row's bytes are overwritten rather
//! than left in free pages (`every_connection_enforces_keys_and_overwrites_what_it_deletes`).
//! A rollback journal rather than a write-ahead log for the same reason: a log
//! keeps old page images until a checkpoint, and a context that erases data
//! for good must not keep a readable copy of it.
//!
//! A schema states its own version, as the one `PRAGMA user_version = N;`
//! its definition runs, so the version is written once, beside the tables it
//! describes (`a_schema_states_its_version_once_in_its_definition`). An empty file is
//! given the schema, in one transaction, at that version. A file at that
//! version is opened. Anything else — another version, or tables
//! with no version — is refused as [`OpenError::Version`] and left untouched
//! (`another_version_is_refused_and_left_as_it_was`). There are no in-place
//! migrations: a schema change bumps the version and ships its own move,
//! decided in its own record, because a reader for an older shape is what
//! "One current contract" forbids.
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::{fmt, io, path::Path};

pub use rusqlite;

/// A context's tables, and the version they are.
#[derive(Clone, Copy, Debug)]
pub struct Schema {
    version: u32,
    definition: &'static str,
}
impl Schema {
    /// `definition` holds the statements that create every table and index,
    /// run once on an empty file, and exactly one `PRAGMA user_version = N;`
    /// on a line of its own, with N above 0 — 0 is an empty file's.
    pub fn new(definition: &'static str) -> Result<Self, OpenError> {
        let mut versions = definition.lines().filter_map(|line| {
            line.trim_matches(|character: char| character.is_ascii_whitespace())
                .strip_prefix("PRAGMA user_version = ")?
                .strip_suffix(';')
        });
        let digits = |version: &str| version.bytes().all(|byte| byte.is_ascii_digit());
        match (
            versions
                .next()
                .filter(|version| digits(version))
                .map(str::parse::<u32>),
            versions.next(),
        ) {
            (Some(Ok(version)), None) if version > 0 => Ok(Self {
                version,
                definition,
            }),
            _ => Err(OpenError::UnversionedSchema),
        }
    }
    /// The version the definition states.
    pub fn version(&self) -> u32 {
        self.version
    }
}

/// Why a database was not opened.
#[derive(Debug)]
pub enum OpenError {
    /// The directory is missing, or not private and this OS user's.
    Directory(io::Error),
    /// The file could not be created or opened privately.
    File(io::Error),
    /// SQLite refused the file, a pragma, or the schema.
    Database(rusqlite::Error),
    /// The file holds tables at a version other than the one asked for;
    /// `found` is 0 when it has tables and no version at all.
    Version { found: u32, expected: u32 },
    /// A definition that does not state one version above 0, which could not
    /// be told from an empty file.
    UnversionedSchema,
}
impl fmt::Display for OpenError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory(error) => write!(output, "database directory: {error}"),
            Self::File(error) => write!(output, "database file: {error}"),
            Self::Database(error) => write!(output, "database: {error}"),
            Self::Version { found, expected } => write!(
                output,
                "database is at schema version {found}, and this build reads only {expected}"
            ),
            Self::UnversionedSchema => output.write_str(
                "a schema must state one version above 0, as `PRAGMA user_version = N;`",
            ),
        }
    }
}
impl std::error::Error for OpenError {}
impl From<rusqlite::Error> for OpenError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

/// Open the database at `path`, creating it privately and giving it `schema`
/// when it is absent or empty.
pub fn open(path: &Path, schema: &Schema) -> Result<Connection, OpenError> {
    let directory = path
        .parent()
        .ok_or_else(|| OpenError::Directory(io::ErrorKind::NotFound.into()))?;
    nessa_local_storage::verify_directory(directory).map_err(OpenError::Directory)?;
    // Private before SQLite opens it, and never through a link. SQLite is
    // then not allowed to create it, so a file removed in between is an error
    // rather than a new one made with the process's own mode.
    drop(
        nessa_local_storage::open(path, nessa_local_storage::OpenMode::OpenOrCreate)
            .map_err(OpenError::File)?,
    );
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "secure_delete", true)?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    // On macOS a plain fsync leaves the write in the drive's cache; the
    // private files these replace were flushed past it, and so is this.
    connection.pragma_update(None, "fullfsync", true)?;
    // A file left in write-ahead mode is brought back to the rollback
    // journal (`a_file_left_in_write_ahead_mode_is_opened_in_the_rollback_journal`).
    connection.pragma_update_and_check(None, "journal_mode", "DELETE", |_| Ok(()))?;
    // Read and, when empty, given its schema under one write lock, so two
    // openers of an empty file cannot both set about creating it.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found: u32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found == schema.version {
        drop(transaction);
        return Ok(connection);
    }
    let tables: u32 =
        transaction.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
    if found != 0 || tables != 0 {
        return Err(OpenError::Version {
            found,
            expected: schema.version,
        });
    }
    // The definition sets its own version, inside the same transaction, and
    // what it set is what it said it would.
    transaction.execute_batch(schema.definition)?;
    let applied: u32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if applied != schema.version {
        return Err(OpenError::Version {
            found: applied,
            expected: schema.version,
        });
    }
    transaction.commit()?;
    Ok(connection)
}
