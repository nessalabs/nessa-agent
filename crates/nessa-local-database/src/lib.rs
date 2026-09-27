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
//! user's; the file is created private when absent, and checked private and
//! not a link, before SQLite opens it by name
//! (`the_file_is_created_private_before_sqlite_opens_it`). That second open is
//! by path, so a process running as this same user could swap the name in
//! between; it is not guarded against, because such a process can already
//! read and write the file itself. What this keeps out is every other user.
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
//! (`another_version_is_refused_and_left_as_it_was`); a file that is not a
//! database is [`OpenError::Unreadable`], and one at this version whose pages
//! `quick_check` finds damaged is [`OpenError::Damaged`]. Those are what
//! the file holds, so opening again cannot change them, and a caller can tell
//! them from a failure that can clear (docs/adr/todo/202-versioned-local-datasets.md).
//! There are no in-place migrations: during alpha a schema change bumps the
//! version and nothing moves the old file, because a reader for an older shape
//! is what "One current contract" forbids.
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
    /// The file is not a database, or SQLite found it damaged. Opening it
    /// again reads the same bytes
    /// (`a_file_that_is_not_a_database_is_refused_and_left_as_it_was`).
    Unreadable(rusqlite::Error),
    /// The file is at this schema's version, but SQLite's `quick_check`
    /// found damage in it; the first thing it reported. Like `Unreadable`,
    /// opening again finds the same
    /// (`a_current_file_with_a_damaged_page_is_refused_and_left_as_it_was`).
    Damaged(String),
    /// The file holds tables at a version other than the one asked for;
    /// `found` is 0 when it has tables and no version at all, and may be any
    /// integer SQLite holds, negative ones included.
    Version { found: i64, expected: u32 },
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
            Self::Unreadable(error) => write!(output, "not a readable database: {error}"),
            Self::Damaged(problem) => write!(output, "damaged database: {problem}"),
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
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt) => {
                Self::Unreadable(error)
            }
            _ => Self::Database(error),
        }
    }
}

#[derive(PartialEq)]
enum Accepted {
    /// At the schema's version.
    Current,
    /// No version and no tables: the schema is still to be given.
    Empty,
}

/// Whether the file is one this schema opens, or refused as another version.
fn accepted(connection: &Connection, schema: &Schema) -> Result<Accepted, OpenError> {
    // Read as SQLite keeps it, a signed integer, so a negative one is another
    // version rather than a value this code cannot hold.
    let found: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found == i64::from(schema.version) {
        return Ok(Accepted::Current);
    }
    let tables: u32 =
        connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
    if found != 0 || tables != 0 {
        return Err(OpenError::Version {
            found,
            expected: schema.version,
        });
    }
    Ok(Accepted::Empty)
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
    // A file at another version is refused before anything that persists is
    // changed in it, its journal mode included, so it is left as it was
    // (`another_version_is_refused_and_left_as_it_was`).
    accepted(&connection, schema)?;
    // A file left in write-ahead mode is brought back to the rollback
    // journal (`a_file_left_in_write_ahead_mode_is_opened_in_the_rollback_journal`).
    connection.pragma_update_and_check(None, "journal_mode", "DELETE", |_| Ok(()))?;
    // Asked again, and when empty given its schema, under one write lock, so
    // two openers of an empty file cannot both set about creating it.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if accepted(&transaction, schema)? == Accepted::Current {
        // A header can be whole over a damaged page that nothing above
        // reads. Every page is read once here, so such a file is refused at
        // open rather than on the first question that reaches it.
        let problem: String =
            transaction.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        if problem != "ok" {
            return Err(OpenError::Damaged(problem));
        }
        drop(transaction);
        return Ok(connection);
    }
    // The definition sets its own version, inside the same transaction, and
    // what it set is what it said it would.
    transaction.execute_batch(schema.definition)?;
    let applied: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if applied != i64::from(schema.version) {
        return Err(OpenError::Version {
            found: applied,
            expected: schema.version,
        });
    }
    transaction.commit()?;
    Ok(connection)
}
