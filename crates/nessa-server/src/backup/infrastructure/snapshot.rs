//! A SQLite read snapshot. Opened while admission is held; copied afterwards.

use std::fs;
use std::path::Path;

use nessa_local_database::rusqlite::{Connection, OpenFlags};
use uuid::Uuid;

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

    /// Copy this snapshot to bytes. The live file is not read again.
    pub fn backup_bytes(&self) -> Result<Vec<u8>, ()> {
        let path = std::env::temp_dir().join(format!(
            "nessa-export-{}-{}.sqlite3",
            std::process::id(),
            Uuid::new_v4()
        ));
        let copied = self.connection.backup("main", &path, None);
        let bytes = match copied {
            Ok(()) => fs::read(&path).map_err(|_| ()),
            Err(_) => Err(()),
        };
        let _ = fs::remove_file(&path);
        bytes
    }
}
