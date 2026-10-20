//! A SQLite read snapshot. Opened while admission is held; copied afterwards.

use std::io::Read;
use std::path::Path;

use nessa_local_database::rusqlite::backup::{Backup, StepResult};
use nessa_local_database::rusqlite::ffi::{self, sqlite3_int64};
use nessa_local_database::rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

/// A session file larger than this is named and left out of the cut.
///
/// The figure is `page_count * page_size` on the snapshot, which is the
/// number of bytes a backup would copy, including free pages.
pub const SESSION_EXPORT_LIMIT: u64 = 64 * 1024 * 1024;

/// Most bytes a public file is read for during the admission hold.
pub const PUBLIC_FILE_LIMIT: u64 = 8 * 1024 * 1024;

/// One conversation named from an oversized session file, without its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredStream {
    pub conversation_id: String,
    pub size: u64,
    /// SHA-256 of the incarnation, floor, tail, and records. See [`stream_digest`].
    pub digest: [u8; 32],
}

/// Bytes copied from a snapshot, or conversations left for a later fetch.
pub enum SnapshotCopy {
    /// The vacuumed database image. Free pages are not in these bytes.
    Ready(Vec<u8>),
    /// The file is above [`SESSION_EXPORT_LIMIT`]. These name its conversations.
    /// Their records were not copied.
    Deferred(Vec<DeferredStream>),
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
    /// A file above [`SESSION_EXPORT_LIMIT`] is [`SnapshotCopy::Deferred`]:
    /// each active conversation is named with its size and a digest of its
    /// stream, and the file is not copied. Otherwise the copy is vacuumed so
    /// deleted rows' free pages are dropped, then the in-memory image is
    /// released before the published bytes are built. Nothing is written to
    /// a shared temp directory. The in-memory connection is dropped on every
    /// return, including a panic while the bytes are being built.
    pub fn backup_bytes(&self) -> Result<SnapshotCopy, ()> {
        if snapshot_bytes(&self.connection)? > SESSION_EXPORT_LIMIT {
            return Ok(SnapshotCopy::Deferred(deferred_streams(&self.connection)?));
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

/// Active conversations in an event-stream session file.
///
/// `public_id` is the conversation id. `size` is the sum of that stream's
/// record payloads. The digest binds the incarnation, the floor, the tail,
/// and each record in offset order: offset, schema id, schema version, event
/// id, and payload, each length-prefixed. A later append changes the tail and
/// the digest. This does not copy the database.
fn deferred_streams(connection: &Connection) -> Result<Vec<DeferredStream>, ()> {
    let mut statement = connection
        .prepare(
            "SELECT n.public_id, s.stream_key, s.incarnation, s.floor, s.tail
             FROM event_stream_names n
             JOIN event_streams s ON s.stream_key = n.active_stream_key
             ORDER BY n.public_id",
        )
        .map_err(|_| ())?;
    let heads = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    drop(statement);
    let mut streams = Vec::with_capacity(heads.len());
    for (conversation_id, stream_key, incarnation, floor, tail) in heads {
        if incarnation.len() != 16 || floor.len() != 8 || tail.len() != 8 {
            return Err(());
        }
        streams.push(DeferredStream {
            conversation_id,
            size: stream_size(connection, stream_key)?,
            digest: stream_digest(connection, stream_key, &incarnation, &floor, &tail)?,
        });
    }
    Ok(streams)
}

fn stream_size(connection: &Connection, stream_key: i64) -> Result<u64, ()> {
    let size: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(length(payload)), 0)
             FROM event_records WHERE stream_key = ?1",
            [stream_key],
            |row| row.get(0),
        )
        .map_err(|_| ())?;
    u64::try_from(size).map_err(|_| ())
}

struct DigestRecord {
    rowid: i64,
    offset: Vec<u8>,
    schema_id: String,
    schema_version: i64,
    event_id: String,
}

fn stream_digest(
    connection: &Connection,
    stream_key: i64,
    incarnation: &[u8],
    floor: &[u8],
    tail: &[u8],
) -> Result<[u8; 32], ()> {
    let mut hasher = Sha256::new();
    hasher.update(b"deferred-session/1");
    hasher.update(incarnation);
    hasher.update(floor);
    hasher.update(tail);
    // Row heads only. Each payload is hashed from the blob API below, so a
    // record larger than the session cap is not loaded into one buffer.
    let records = digest_records(connection, stream_key)?;
    for record in records {
        hasher.update(&record.offset);
        write_prefixed(&mut hasher, record.schema_id.as_bytes())?;
        hasher.update(record.schema_version.to_be_bytes());
        write_prefixed(&mut hasher, record.event_id.as_bytes())?;
        hash_payload(connection, record.rowid, &mut hasher)?;
    }
    Ok(hasher.finalize().into())
}

fn digest_records(connection: &Connection, stream_key: i64) -> Result<Vec<DigestRecord>, ()> {
    let mut statement = connection
        .prepare(
            "SELECT rowid, offset, schema_id, schema_version, event_id
             FROM event_records WHERE stream_key = ?1 ORDER BY offset",
        )
        .map_err(|_| ())?;
    let records = statement
        .query_map([stream_key], |row| {
            Ok(DigestRecord {
                rowid: row.get(0)?,
                offset: row.get(1)?,
                schema_id: row.get(2)?,
                schema_version: row.get(3)?,
                event_id: row.get(4)?,
            })
        })
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    Ok(records)
}

/// Hash the payload of `rowid` in chunks. `length` is written before the
/// bytes, which is the same layout as hashing the value in one buffer.
fn hash_payload(connection: &Connection, rowid: i64, hasher: &mut Sha256) -> Result<(), ()> {
    let mut blob = connection
        .blob_open("main", "event_records", "payload", rowid, true)
        .map_err(|_| ())?;
    let len = u64::try_from(blob.len()).map_err(|_| ())?;
    hasher.update(len.to_be_bytes());
    let mut buf = [0u8; 64 * 1024];
    let mut hashed = 0u64;
    loop {
        let read = blob.read(&mut buf).map_err(|_| ())?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        hashed = hashed
            .checked_add(u64::try_from(read).map_err(|_| ())?)
            .ok_or(())?;
    }
    if hashed != len {
        return Err(());
    }
    Ok(())
}

fn write_prefixed(hasher: &mut Sha256, bytes: &[u8]) -> Result<(), ()> {
    let len = u32::try_from(bytes.len()).map_err(|_| ())?;
    hasher.update(len.to_be_bytes());
    hasher.update(bytes);
    Ok(())
}

/// Why a deferred conversation could not be read or copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterializeError {
    Missing,
    Interrupted,
    Unreadable,
}

/// Size and stream digest of one conversation, from the same inputs the export uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MaterializedStream {
    pub size: u64,
    pub digest: [u8; 32],
}

/// Read one conversation's stream fingerprint. A missing file or a missing
/// conversation is [`MaterializeError::Missing`].
pub(crate) fn inspect_conversation(
    path: &Path,
    conversation_id: &str,
) -> Result<MaterializedStream, MaterializeError> {
    if !path.is_file() {
        return Err(MaterializeError::Missing);
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| MaterializeError::Unreadable)?;
    let (_key, stream) = one_stream(&connection, conversation_id)?;
    Ok(MaterializedStream {
        size: stream.size,
        digest: stream.digest,
    })
}

/// Copy one conversation into `destination`, streaming each payload.
///
/// The destination is removed if the copy does not finish or its digest does
/// not match the source. `progress` is `(copied, total)` payload bytes.
pub(crate) fn materialize_conversation(
    source: &Path,
    conversation_id: &str,
    destination: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<MaterializedStream, MaterializeError> {
    if !source.is_file() {
        return Err(MaterializeError::Missing);
    }
    let connection = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| MaterializeError::Unreadable)?;
    let (stream_key, stream) = one_stream(&connection, conversation_id)?;
    progress(0, stream.size);
    let mut created = CreatedDestination::new(destination)?;
    copy_stream(&connection, stream_key, &stream, created.path, progress)?;
    let copied = inspect_conversation(created.path, conversation_id)?;
    if copied.size != stream.size || copied.digest != stream.digest {
        return Err(MaterializeError::Unreadable);
    }
    created.keep = true;
    Ok(copied)
}

struct CreatedDestination<'a> {
    path: &'a Path,
    keep: bool,
}

impl<'a> CreatedDestination<'a> {
    fn new(path: &'a Path) -> Result<Self, MaterializeError> {
        if path.exists() {
            return Err(MaterializeError::Unreadable);
        }
        Ok(Self { path, keep: false })
    }
}

impl Drop for CreatedDestination<'_> {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

fn one_stream(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(i64, DeferredStream), MaterializeError> {
    let found = connection.query_row(
        "SELECT s.stream_key, s.incarnation, s.floor, s.tail
         FROM event_stream_names n
         JOIN event_streams s ON s.stream_key = n.active_stream_key
         WHERE n.public_id = ?1",
        [conversation_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        },
    );
    let (stream_key, incarnation, floor, tail) = match found {
        Ok(row) => row,
        Err(nessa_local_database::rusqlite::Error::QueryReturnedNoRows) => {
            return Err(MaterializeError::Missing);
        }
        Err(_) => return Err(MaterializeError::Unreadable),
    };
    if incarnation.len() != 16 || floor.len() != 8 || tail.len() != 8 {
        return Err(MaterializeError::Unreadable);
    }
    let size = stream_size(connection, stream_key).map_err(|_| MaterializeError::Interrupted)?;
    let digest = stream_digest(connection, stream_key, &incarnation, &floor, &tail)
        .map_err(|_| MaterializeError::Interrupted)?;
    Ok((
        stream_key,
        DeferredStream {
            conversation_id: conversation_id.to_owned(),
            size,
            digest,
        },
    ))
}

const SESSION_SCHEMA: &str = "
CREATE TABLE event_streams(
    stream_key INTEGER PRIMARY KEY,
    public_id TEXT NOT NULL,
    incarnation BLOB NOT NULL,
    floor BLOB NOT NULL,
    tail BLOB NOT NULL,
    retired INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE event_stream_names(
    public_id TEXT PRIMARY KEY,
    latest_incarnation BLOB NOT NULL,
    active_stream_key INTEGER
);
CREATE TABLE event_records(
    stream_key INTEGER NOT NULL,
    offset BLOB NOT NULL,
    event_id TEXT NOT NULL,
    schema_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    payload BLOB NOT NULL
);";

fn copy_stream(
    source: &Connection,
    stream_key: i64,
    stream: &DeferredStream,
    destination: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), MaterializeError> {
    let (incarnation, floor, tail) = source
        .query_row(
            "SELECT incarnation, floor, tail FROM event_streams WHERE stream_key = ?1",
            [stream_key],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .map_err(|_| MaterializeError::Interrupted)?;
    let records = digest_records(source, stream_key).map_err(|_| MaterializeError::Interrupted)?;
    let dest = Connection::open(destination).map_err(|_| MaterializeError::Unreadable)?;
    dest.execute_batch(SESSION_SCHEMA)
        .map_err(|_| MaterializeError::Unreadable)?;
    dest.execute(
        "INSERT INTO event_streams(stream_key, public_id, incarnation, floor, tail)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        nessa_local_database::rusqlite::params![
            stream_key,
            stream.conversation_id,
            incarnation,
            floor,
            tail
        ],
    )
    .map_err(|_| MaterializeError::Unreadable)?;
    dest.execute(
        "INSERT INTO event_stream_names(public_id, latest_incarnation, active_stream_key)
         VALUES (?1, ?2, ?3)",
        nessa_local_database::rusqlite::params![stream.conversation_id, incarnation, stream_key],
    )
    .map_err(|_| MaterializeError::Unreadable)?;
    let mut copied = 0u64;
    for record in records {
        let length: i64 = source
            .query_row(
                "SELECT length(payload) FROM event_records WHERE rowid = ?1",
                [record.rowid],
                |row| row.get(0),
            )
            .map_err(|_| MaterializeError::Interrupted)?;
        dest.execute(
            "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, zeroblob(?6))",
            nessa_local_database::rusqlite::params![
                stream_key,
                record.offset,
                record.event_id,
                record.schema_id,
                record.schema_version,
                length
            ],
        )
        .map_err(|_| MaterializeError::Unreadable)?;
        let dest_row = dest.last_insert_rowid();
        copy_blob(
            source,
            record.rowid,
            &dest,
            dest_row,
            &mut copied,
            stream.size,
            progress,
        )?;
    }
    if copied != stream.size {
        return Err(MaterializeError::Interrupted);
    }
    progress(copied, stream.size);
    Ok(())
}

fn copy_blob(
    source: &Connection,
    source_row: i64,
    dest: &Connection,
    dest_row: i64,
    copied: &mut u64,
    total: u64,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), MaterializeError> {
    let mut from = source
        .blob_open("main", "event_records", "payload", source_row, true)
        .map_err(|_| MaterializeError::Interrupted)?;
    let mut into = dest
        .blob_open("main", "event_records", "payload", dest_row, false)
        .map_err(|_| MaterializeError::Unreadable)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = from
            .read(&mut buf)
            .map_err(|_| MaterializeError::Interrupted)?;
        if read == 0 {
            break;
        }
        use std::io::Write;
        into.write_all(&buf[..read])
            .map_err(|_| MaterializeError::Interrupted)?;
        *copied = copied
            .checked_add(u64::try_from(read).map_err(|_| MaterializeError::Interrupted)?)
            .ok_or(MaterializeError::Interrupted)?;
        progress(*copied, total);
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deferred_digest_hashes_each_payload_in_chunks() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE event_records(
                    stream_key INTEGER NOT NULL,
                    offset BLOB NOT NULL,
                    event_id TEXT NOT NULL,
                    schema_id TEXT NOT NULL,
                    schema_version INTEGER NOT NULL,
                    payload BLOB NOT NULL
                );",
            )
            .unwrap();
        let wide = vec![0xA5u8; 70_000];
        let offset_wide = [0u8, 0, 0, 0, 0, 0, 0, 1];
        let offset_zero = [0u8, 0, 0, 0, 0, 0, 0, 2];
        connection
            .execute(
                "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
                 VALUES (7, ?1, 'wide', 'nessa.record', 3, ?2)",
                nessa_local_database::rusqlite::params![offset_wide.as_slice(), wide.as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
                 VALUES (7, ?1, 'zero', 'nessa.record', 4, zeroblob(1000))",
                [offset_zero.as_slice()],
            )
            .unwrap();
        let incarnation = [9u8; 16];
        let floor = [1u8; 8];
        let tail = [2u8; 8];
        let digest = stream_digest(&connection, 7, &incarnation, &floor, &tail).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(b"deferred-session/1");
        hasher.update(incarnation);
        hasher.update(floor);
        hasher.update(tail);
        expect_record(
            &mut hasher,
            Expected {
                offset: &offset_wide,
                schema_id: "nessa.record",
                version: 3,
                event_id: "wide",
                payload: &wide,
            },
        );
        let zeros = [0u8; 1000];
        expect_record(
            &mut hasher,
            Expected {
                offset: &offset_zero,
                schema_id: "nessa.record",
                version: 4,
                event_id: "zero",
                payload: &zeros,
            },
        );
        let expected: [u8; 32] = hasher.finalize().into();
        assert_eq!(digest, expected);
    }

    struct Expected<'a> {
        offset: &'a [u8],
        schema_id: &'a str,
        version: i64,
        event_id: &'a str,
        payload: &'a [u8],
    }

    fn expect_record(hasher: &mut Sha256, record: Expected<'_>) {
        hasher.update(record.offset);
        hasher.update(u32::try_from(record.schema_id.len()).unwrap().to_be_bytes());
        hasher.update(record.schema_id.as_bytes());
        hasher.update(record.version.to_be_bytes());
        hasher.update(u32::try_from(record.event_id.len()).unwrap().to_be_bytes());
        hasher.update(record.event_id.as_bytes());
        hasher.update(u64::try_from(record.payload.len()).unwrap().to_be_bytes());
        hasher.update(record.payload);
    }
}
