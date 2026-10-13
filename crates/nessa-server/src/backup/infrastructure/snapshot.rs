//! A SQLite read snapshot. Opened while admission is held; copied afterwards.

use std::path::Path;

use nessa_local_database::rusqlite::backup::{Backup, StepResult};
use nessa_local_database::rusqlite::{Connection, OpenFlags};

/// A read transaction on one database file.
///
/// The first read starts the snapshot. A commit that lands after [`Self::open`]
/// is not visible here, including to [`Self::backup_bytes`]. Dropping this
/// releases the database's shared lock.
pub struct SqliteSnapshot {
    connection: Connection,
}

impl SqliteSnapshot {
    /// Open `path` read-only and begin a read transaction. A missing or
    /// unreadable file is `Err`. Nothing is created.
    pub fn open(path: &Path) -> Result<Self, ()> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| ())?;
        connection.execute_batch("BEGIN").map_err(|_| ())?;
        // The snapshot starts at the first read, which is this one, still
        // inside the admission hold.
        connection
            .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
            .map_err(|_| ())?;
        Ok(Self { connection })
    }

    /// Copy this snapshot into memory. The live file is not read again.
    ///
    /// The copy is one `Vec<u8>`: the published record is the whole file, and
    /// the publisher takes those bytes together. Nothing is written to a
    /// shared temp directory. The in-memory connection is dropped on every
    /// return, including a panic while the bytes are being built.
    pub fn backup_bytes(&self) -> Result<Vec<u8>, ()> {
        let mut memory = Connection::open_in_memory().map_err(|_| ())?;
        {
            let backup = Backup::new(&self.connection, &mut memory).map_err(|_| ())?;
            if !matches!(backup.step(-1).map_err(|_| ())?, StepResult::Done) {
                return Err(());
            }
        }
        let data = memory.serialize("main").map_err(|_| ())?;
        Ok(data.to_vec())
    }
}
