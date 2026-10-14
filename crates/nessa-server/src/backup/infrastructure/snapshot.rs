//! A SQLite read snapshot. Opened while admission is held; copied afterwards.

use std::path::Path;

use nessa_local_database::rusqlite::backup::{Backup, StepResult};
use nessa_local_database::rusqlite::ffi::{self, sqlite3_int64};
use nessa_local_database::rusqlite::{Connection, OpenFlags};

/// A session file larger than this is named and left out of the cut.
///
/// The figure is `page_count * page_size` on the snapshot, which is the
/// number of bytes a backup would copy, including free pages.
pub const SESSION_EXPORT_LIMIT: u64 = 64 * 1024 * 1024;

/// Bytes copied from a snapshot, or a file left unread because it is too big.
pub enum SnapshotCopy {
    /// The vacuumed database image. Free pages are not in these bytes.
    Ready(Vec<u8>),
    /// `page_count * page_size` is above [`SESSION_EXPORT_LIMIT`].
    Oversized,
}

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
    /// A file above [`SESSION_EXPORT_LIMIT`] is [`SnapshotCopy::Oversized`]
    /// and is not copied. Otherwise the copy is vacuumed so deleted rows'
    /// free pages are dropped, then the in-memory image is released before
    /// the published bytes are built. Nothing is written to a shared temp
    /// directory. The in-memory connection is dropped on every return,
    /// including a panic while the bytes are being built.
    pub fn backup_bytes(&self) -> Result<SnapshotCopy, ()> {
        if snapshot_bytes(&self.connection)? > SESSION_EXPORT_LIMIT {
            return Ok(SnapshotCopy::Oversized);
        }
        let mut memory = Connection::open_in_memory().map_err(|_| ())?;
        {
            let backup = Backup::new(&self.connection, &mut memory).map_err(|_| ())?;
            if !matches!(backup.step(-1).map_err(|_| ())?, StepResult::Done) {
                return Err(());
            }
        }
        // Online backup copies free pages. Vacuum rebuilds the image from
        // the live rows, and secure_delete overwrites pages that vacuum frees.
        memory
            .execute_batch("PRAGMA secure_delete = ON; VACUUM;")
            .map_err(|_| ())?;
        Ok(SnapshotCopy::Ready(serialized_image(memory)?))
    }
}

fn snapshot_bytes(connection: &Connection) -> Result<u64, ()> {
    let pages: i64 = connection
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .map_err(|_| ())?;
    let page_size: i64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|_| ())?;
    if pages < 0 || page_size < 0 {
        return Err(());
    }
    Ok((pages as u64).saturating_mul(page_size as u64))
}

/// `connection` is the in-memory copy. It is dropped before the returned
/// buffer is allocated, so the image and the published bytes are not live
/// together.
fn serialized_image(connection: Connection) -> Result<Vec<u8>, ()> {
    let mut size: sqlite3_int64 = 0;
    // SAFETY: `handle` is this open connection. Flag 0 allocates a new
    // buffer with sqlite3_malloc. That buffer stays valid after the
    // connection is closed and is released with sqlite3_free.
    let ptr =
        unsafe { ffi::sqlite3_serialize(connection.handle(), c"main".as_ptr(), &mut size, 0) };
    drop(connection);
    if ptr.is_null() {
        return Err(());
    }
    let buffer = SqliteMalloc(ptr);
    let len = usize::try_from(size).map_err(|_| ())?;
    if len == 0 {
        return Err(());
    }
    // SAFETY: `buffer` is `len` bytes allocated by sqlite3_serialize.
    Ok(unsafe { std::slice::from_raw_parts(buffer.0, len) }.to_vec())
}

/// An image allocated by `sqlite3_malloc`. Drop frees it, including on panic.
struct SqliteMalloc(*mut u8);

impl Drop for SqliteMalloc {
    fn drop(&mut self) {
        // SAFETY: the pointer came from sqlite3_serialize and is freed once.
        unsafe { ffi::sqlite3_free(self.0.cast()) };
    }
}
