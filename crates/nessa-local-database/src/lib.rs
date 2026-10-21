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
//!
//! A schema may carry [`Upgrade`]s: forward-only steps, each from one older
//! version to the next. A file at a version some step starts from is
//! checked as above, then brought to the schema's version by every step from
//! there on, in the same transaction that would have created it, so a crash
//! or a failed step leaves it at the version it had
//! (`an_older_file_is_upgraded_in_one_transaction_and_keeps_its_rows`). A
//! step that fails is [`OpenError::Upgrade`], naming that step and its own
//! cause. A version no step starts from is refused as above. Nothing reads
//! an older shape: an upgrade brings the file to the current one first
//! (docs/adr/todo/202-versioned-local-datasets.md, "Migrations").
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::{fmt, io, path::Path};

pub use rusqlite;

/// A context's tables, and the version they are.
#[derive(Clone, Copy, Debug)]
pub struct Schema {
    version: u32,
    definition: &'static str,
    upgrades: &'static [Upgrade],
}
impl Schema {
    /// `definition` holds the statements that create every table and index,
    /// run once on an empty file, and exactly one `PRAGMA user_version = N;`
    /// on a line of its own, with N above 0 — 0 is an empty file's.
    pub fn new(definition: &'static str) -> Result<Self, OpenError> {
        Ok(Self {
            version: stated_version(definition).ok_or(OpenError::UnversionedSchema)?,
            definition,
            upgrades: &[],
        })
    }
    /// The steps that bring an older file to this version, oldest first. Each
    /// states the version it sets, one above the one before it, and the last
    /// sets this schema's own (`upgrades_must_chain_to_the_schemas_version`).
    pub fn with_upgrades(self, upgrades: &'static [Upgrade]) -> Result<Self, OpenError> {
        let mut next = upgrades.first().map(|upgrade| upgrade.from);
        for upgrade in upgrades {
            let reached = stated_version(upgrade.statements);
            if Some(upgrade.from) != next || reached != upgrade.from.checked_add(1) {
                return Err(OpenError::UnversionedSchema);
            }
            next = reached;
        }
        if next.is_some_and(|reached| reached != self.version) {
            return Err(OpenError::UnversionedSchema);
        }
        Ok(Self { upgrades, ..self })
    }
    /// The version the definition states.
    pub fn version(&self) -> u32 {
        self.version
    }
    /// The steps from `found` on, when some step starts there.
    fn upgrades_from(&self, found: i64) -> Option<&'static [Upgrade]> {
        let first = self
            .upgrades
            .iter()
            .position(|upgrade| i64::from(upgrade.from) == found)?;
        Some(&self.upgrades[first..])
    }
}

/// One forward step: the statements that bring a file at `from` to the next
/// version, ending with `PRAGMA user_version = from + 1;` on a line of its
/// own. A step only adds; what it adds is exactly what the current
/// definition creates, so an upgraded file and a new one are the same shape.
#[derive(Clone, Copy, Debug)]
pub struct Upgrade {
    from: u32,
    statements: &'static str,
}
impl Upgrade {
    pub const fn new(from: u32, statements: &'static str) -> Self {
        Self { from, statements }
    }
}

/// The one `PRAGMA user_version = N;` line in `statements`, N above 0.
fn stated_version(statements: &str) -> Option<u32> {
    let mut versions = statements.lines().filter_map(|line| {
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
        (Some(Ok(version)), None) if version > 0 => Some(version),
        _ => None,
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
    /// A step that brings an older file forward failed; `from` is the
    /// version that step starts at, and the file is left at the version it
    /// had. `cause` is SQLite's own: a busy or I/O failure can clear, and a
    /// file the step cannot apply to (a table already there) cannot.
    Upgrade { from: u32, cause: rusqlite::Error },
    /// A definition that does not state one version above 0, which could not
    /// be told from an empty file, or upgrades that do not step one version
    /// at a time up to it.
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
            Self::Upgrade { from, cause } => write!(
                output,
                "database could not be upgraded from schema version {from}: {cause}"
            ),
            Self::UnversionedSchema => output.write_str(
                "a schema must state one version above 0, as `PRAGMA user_version = N;`, \
                 and its upgrades must step one version at a time up to it",
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

enum Accepted {
    /// At the schema's version.
    Current,
    /// At an older version these steps bring to the schema's.
    Older(&'static [Upgrade]),
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
    if let Some(upgrades) = schema.upgrades_from(found) {
        return Ok(Accepted::Older(upgrades));
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
    open_mode(path, schema, nessa_local_storage::OpenMode::OpenOrCreate)
}

/// Open the database at `path` only when that file is already there.
///
/// A missing file is [`OpenError::File`] with [`io::ErrorKind::NotFound`], and
/// nothing is created. An empty file that is already there is given `schema`,
/// the same way [`open`] does.
pub fn open_existing(path: &Path, schema: &Schema) -> Result<Connection, OpenError> {
    open_mode(path, schema, nessa_local_storage::OpenMode::ReadWrite)
}

fn open_mode(
    path: &Path,
    schema: &Schema,
    mode: nessa_local_storage::OpenMode,
) -> Result<Connection, OpenError> {
    let directory = path
        .parent()
        .ok_or_else(|| OpenError::Directory(io::ErrorKind::NotFound.into()))?;
    nessa_local_storage::verify_directory(directory).map_err(OpenError::Directory)?;
    // Private before SQLite opens it, and never through a link. SQLite is
    // then not allowed to create it, so a file removed in between is an error
    // rather than a new one made with the process's own mode. `ReadWrite`
    // does not create a file that was already absent.
    drop(nessa_local_storage::open(path, mode).map_err(OpenError::File)?);
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
    // An older file is upgraded under the same lock, so a second opener
    // waits and then finds it current.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let upgrades = match accepted(&transaction, schema)? {
        Accepted::Empty => None,
        Accepted::Current => Some(&[][..]),
        Accepted::Older(upgrades) => Some(upgrades),
    };
    if let Some(upgrades) = upgrades {
        // A header can be whole over a damaged page that nothing above
        // reads. Every page is read once here, so such a file is refused at
        // open rather than on the first question that reaches it, and an
        // older one before any step writes to it.
        let problem: String =
            transaction.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        if problem != "ok" {
            return Err(OpenError::Damaged(problem));
        }
        if upgrades.is_empty() {
            drop(transaction);
            return Ok(connection);
        }
        // Every step or none: dropping the transaction on an error rolls
        // back the steps already run, and the file keeps the version it had.
        for upgrade in upgrades {
            let failed = |cause| OpenError::Upgrade {
                from: upgrade.from,
                cause,
            };
            transaction
                .execute_batch(upgrade.statements)
                .map_err(failed)?;
            let reached: i64 = transaction
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .map_err(failed)?;
            if reached != i64::from(upgrade.from) + 1 {
                return Err(OpenError::Version {
                    found: reached,
                    expected: schema.version,
                });
            }
        }
        transaction.commit()?;
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
