//! A SQLite read snapshot. Opened while admission is held; copied afterwards.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::snapshot_file::HeldDatabase;
use nessa_local_database::rusqlite::backup::{Backup, StepResult};
use nessa_local_database::rusqlite::ffi::{self, sqlite3_int64};
use nessa_local_database::rusqlite::{Connection, OpenFlags, Transaction};
use sha2::{Digest, Sha256};

/// A session file larger than this is named and left out of the cut.
///
/// The figure is `page_count * page_size` on the snapshot, which is the
/// number of bytes a backup would copy, including free pages.
pub const SESSION_EXPORT_LIMIT: u64 = 64 * 1024 * 1024;

/// Most bytes a public file is read for during the admission hold.
pub const PUBLIC_FILE_LIMIT: u64 = 8 * 1024 * 1024;

/// Restore's per-identifier allocation budget, separate from payload size.
const STREAM_METADATA_LIMIT: i64 = 4096;

// Guard the comparison itself: guarding SELECT outputs would still let SQLite
// allocate non-integer keys while evaluating the join.
const STREAM_SOURCE_SQL: &str = "FROM event_stream_names n
    LEFT JOIN event_streams s ON CASE
        WHEN typeof(s.stream_key)='integer' AND typeof(n.active_stream_key)='integer'
        THEN s.stream_key=n.active_stream_key ELSE 0 END
    WHERE n.active_stream_key IS NOT NULL";

const RECORD_HEAD_SQL: &str = "SELECT rowid, offset,
    CASE WHEN typeof(schema_id)='text' AND octet_length(schema_id) BETWEEN 1 AND ?2 THEN schema_id END,
    CASE WHEN typeof(schema_version)='integer' THEN schema_version END,
    CASE WHEN typeof(event_id)='text' AND octet_length(event_id) BETWEEN 1 AND ?2 THEN event_id END
    FROM event_records WHERE stream_key = ?1 ORDER BY offset";

/// Reuse an export's pinned transaction, or pin these reads ourselves. SQLite
/// sorts may spill to disk with the normal VFS; the held-file VFS refuses such
/// opens, rather than accumulating an image-sized in-memory sorter.
fn stream_read(connection: &Connection) -> Result<Option<Transaction<'_>>, ()> {
    if connection.is_autocommit() {
        connection
            .execute_batch("PRAGMA temp_store=FILE;")
            .map_err(|_| ())?;
        Ok(Some(connection.unchecked_transaction().map_err(|_| ())?))
    } else {
        Ok(None)
    }
}

/// Field-size guards below apply to stored columns, not attacker expressions
/// in views, virtual tables, or generated columns. The implicit rowid must
/// identify the same physical row that the blob API reads.
fn ordinary_stream_table(connection: &Connection, table: &str) -> Result<(), ()> {
    let kind: String = connection
        .query_row(
            "SELECT type FROM pragma_table_list WHERE schema='main' AND name=?1",
            [table],
            |row| row.get(0),
        )
        .map_err(|_| ())?;
    if kind != "table" {
        return Err(());
    }
    let unsupported: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo(?1, 'main')
             WHERE hidden!=0 OR lower(name) IN ('rowid', '_rowid_', 'oid'))",
            [table],
            |row| row.get(0),
        )
        .map_err(|_| ())?;
    if unsupported {
        Err(())
    } else {
        Ok(())
    }
}

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
        // Configure before BEGIN: this bundled SQLite defaults temporary
        // storage to memory, including sorts used while enumerating streams.
        connection
            .execute_batch("PRAGMA temp_store=FILE;")
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
    let _read = stream_read(connection)?;
    ordinary_stream_table(connection, "event_stream_names")?;
    ordinary_stream_table(connection, "event_streams")?;
    let mut statement = connection
        .prepare(&format!(
            "SELECT CASE WHEN typeof(n.public_id)='text' AND octet_length(n.public_id) BETWEEN 1 AND ?1 THEN n.public_id END,
                    CASE WHEN typeof(s.stream_key)='integer' THEN s.stream_key END,
                    CASE WHEN typeof(s.incarnation)='blob' AND octet_length(s.incarnation)=16 THEN s.incarnation END,
                    CASE WHEN typeof(s.floor)='blob' AND octet_length(s.floor)=8 THEN s.floor END,
                    CASE WHEN typeof(s.tail)='blob' AND octet_length(s.tail)=8 THEN s.tail END
             {STREAM_SOURCE_SQL}
             ORDER BY 1",
        ))
        .map_err(|_| ())?;
    let heads = statement
        .query_map([STREAM_METADATA_LIMIT], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(|_| ())?;
    let mut streams = Vec::new();
    for head in heads {
        let (conversation_id, stream_key, incarnation, floor, tail) = head.map_err(|_| ())?;
        streams.push(DeferredStream {
            conversation_id,
            size: stream_size(connection, stream_key)?,
            digest: stream_digest(connection, stream_key, &incarnation, &floor, &tail)?,
        });
    }
    Ok(streams)
}

fn stream_size(connection: &Connection, stream_key: i64) -> Result<u64, ()> {
    ordinary_stream_table(connection, "event_records")?;
    let invalid: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM event_records WHERE stream_key=?1 AND typeof(payload)!='blob')",
        [stream_key], |row| row.get(0),
    ).map_err(|_| ())?;
    if invalid {
        return Err(());
    }
    let size: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(octet_length(payload)), 0)
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
    visit_digest_records(connection, stream_key, |record| {
        hasher.update(&record.offset);
        write_prefixed(&mut hasher, record.schema_id.as_bytes())?;
        hasher.update(record.schema_version.to_be_bytes());
        write_prefixed(&mut hasher, record.event_id.as_bytes())?;
        hash_payload(connection, record.rowid, &mut hasher)
    })?;
    Ok(hasher.finalize().into())
}

/// Keep one bounded row head alive, regardless of the number of records.
fn visit_digest_records<E: From<()>>(
    connection: &Connection,
    stream_key: i64,
    mut visit: impl FnMut(DigestRecord) -> Result<(), E>,
) -> Result<(), E> {
    let _read = stream_read(connection)?;
    ordinary_stream_table(connection, "event_records")?;
    // Check the ordering key before SQLite can put it into a sorter. The read
    // transaction keeps this check and the following ordered scan together.
    let invalid: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM event_records WHERE stream_key=?1
         AND (typeof(offset)!='blob' OR octet_length(offset)!=8))",
            [stream_key],
            |row| row.get(0),
        )
        .map_err(|_| ())?;
    if invalid {
        return Err(().into());
    }
    let mut statement = connection.prepare(RECORD_HEAD_SQL).map_err(|_| ())?;
    let records = statement
        .query_map([stream_key, STREAM_METADATA_LIMIT], |row| {
            Ok(DigestRecord {
                rowid: row.get(0)?,
                offset: row.get(1)?,
                schema_id: row.get(2)?,
                schema_version: row.get(3)?,
                event_id: row.get(4)?,
            })
        })
        .map_err(|_| ())?;
    for record in records {
        visit(record.map_err(|_| ())?)?;
    }
    Ok(())
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

impl From<()> for MaterializeError {
    fn from(_: ()) -> Self {
        Self::Interrupted
    }
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

/// Stream fingerprint of a database image this process still holds.
///
/// The private read-only VFS reads pages from the retained descriptor, including
/// unnamed or unlinked files, with a one-MiB SQLite page cache. No pathname is
/// reopened while inspecting the held inode. The known stream tables must
/// contain exactly this stream, without another head, name, or orphan record.
/// Publication separately rematerializes that stream into a canonical image.
pub(crate) fn inspect_open_file(
    file: &mut File,
    conversation_id: &str,
) -> Result<MaterializedStream, MaterializeError> {
    let database = HeldDatabase::open(file).map_err(|_| MaterializeError::Unreadable)?;
    let connection = database.connection();
    let _read = stream_read(connection).map_err(|_| MaterializeError::Unreadable)?;
    let (key, stream) = one_stream(connection, conversation_id)?;
    // A held destination is the whole image we will publish. A matching
    // requested stream does not authorize another stream or orphan records.
    // Count at most two heads and inspect record ownership without loading
    // payloads or comparing malformed non-integer keys.
    let exclusive: bool = connection
        .query_row(
            "SELECT
            (SELECT count(*) FROM (SELECT 1 FROM event_streams LIMIT 2))=1
            AND (SELECT count(*) FROM (SELECT 1 FROM event_stream_names LIMIT 2))=1
            AND NOT EXISTS(SELECT 1 FROM event_records WHERE CASE
                WHEN typeof(stream_key)='integer' THEN stream_key!=?1
                ELSE 1 END)",
            [key],
            |row| row.get(0),
        )
        .map_err(|_| MaterializeError::Unreadable)?;
    if !exclusive {
        return Err(MaterializeError::Unreadable);
    }
    Ok(MaterializedStream {
        size: stream.size,
        digest: stream.digest,
    })
}

/// Write the conversation into the exact file the restore attempt reserved.
pub(crate) fn materialize_into_file(
    source: &Path,
    conversation_id: &str,
    destination: &mut File,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<MaterializedStream, MaterializeError> {
    if !source.is_file() {
        return Err(MaterializeError::Missing);
    }
    let source = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| MaterializeError::Unreadable)?;
    materialize_connection_into_file(&source, conversation_id, destination, progress)
}

/// Normalize a held source image into a fresh held destination. Only the
/// requested stream enters the canonical schema; source-side tables, columns,
/// free pages, and trailing bytes are never copied as database-image bytes.
pub(crate) fn materialize_held_into_file(
    source: &mut File,
    conversation_id: &str,
    destination: &mut File,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<MaterializedStream, MaterializeError> {
    let source = HeldDatabase::open(source).map_err(|_| MaterializeError::Unreadable)?;
    materialize_connection_into_file(source.connection(), conversation_id, destination, progress)
}

fn materialize_connection_into_file(
    source: &Connection,
    conversation_id: &str,
    destination: &mut File,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<MaterializedStream, MaterializeError> {
    let _read = stream_read(source).map_err(|_| MaterializeError::Unreadable)?;
    let (key, stream) = one_stream(source, conversation_id)?;
    let dest = HeldDatabase::writable(destination).map_err(|_| MaterializeError::Unreadable)?;
    copy_stream_into(source, key, &stream, dest.connection(), progress)?;
    drop(dest);
    destination
        .sync_all()
        .map_err(|_| MaterializeError::Unreadable)?;
    let copied = inspect_open_file(destination, conversation_id)?;
    if copied.size != stream.size || copied.digest != stream.digest {
        return Err(MaterializeError::Unreadable);
    }
    Ok(copied)
}

fn one_stream(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(i64, DeferredStream), MaterializeError> {
    let _read = stream_read(connection).map_err(|_| MaterializeError::Unreadable)?;
    ordinary_stream_table(connection, "event_stream_names")
        .and_then(|()| ordinary_stream_table(connection, "event_streams"))
        .map_err(|_| MaterializeError::Unreadable)?;
    let found = connection.query_row(
        &format!("SELECT CASE WHEN typeof(s.stream_key)='integer' THEN s.stream_key END,
                CASE WHEN typeof(s.incarnation)='blob' AND octet_length(s.incarnation)=16 THEN s.incarnation END,
                CASE WHEN typeof(s.floor)='blob' AND octet_length(s.floor)=8 THEN s.floor END,
                CASE WHEN typeof(s.tail)='blob' AND octet_length(s.tail)=8 THEN s.tail END
         {STREAM_SOURCE_SQL}
         AND n.public_id = ?1"),
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
);
CREATE INDEX event_records_stream_offset ON event_records(stream_key, offset);";

fn copy_stream_into(
    source: &Connection,
    stream_key: i64,
    stream: &DeferredStream,
    dest: &Connection,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), MaterializeError> {
    let _read = stream_read(source).map_err(|_| MaterializeError::Interrupted)?;
    ordinary_stream_table(source, "event_streams").map_err(|_| MaterializeError::Interrupted)?;
    let (incarnation, floor, tail) = source
        .query_row(
            "SELECT CASE WHEN typeof(incarnation)='blob' AND octet_length(incarnation)=16 THEN incarnation END,
                    CASE WHEN typeof(floor)='blob' AND octet_length(floor)=8 THEN floor END,
                    CASE WHEN typeof(tail)='blob' AND octet_length(tail)=8 THEN tail END
             FROM event_streams WHERE stream_key = ?1",
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
    visit_digest_records(source, stream_key, |record| {
        let length: i64 = source
            .query_row(
                "SELECT CASE WHEN typeof(payload)='blob' THEN octet_length(payload) END
                 FROM event_records WHERE rowid = ?1",
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
            dest,
            dest_row,
            &mut copied,
            stream.size,
            progress,
        )
    })?;
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

/// A session image with fenced conversations removed.
///
/// `None` means no named conversation was in the image, so the caller keeps
/// the authenticated bytes. `Some` is that image after those streams were
/// deleted and the database was vacuumed. An error means the image could
/// not be checked, and the caller must not keep the original bytes.
pub(crate) fn without_fenced_conversations(
    bytes: &[u8],
    fenced: &[String],
) -> Result<Option<Vec<u8>>, MaterializeError> {
    if fenced.is_empty() {
        return Ok(None);
    }
    if !bytes.starts_with(b"SQLite format 3") {
        return Err(MaterializeError::Unreadable);
    }
    let mut source = Connection::open_in_memory().map_err(|_| MaterializeError::Unreadable)?;
    source
        .deserialize_read_exact("main", std::io::Cursor::new(bytes), bytes.len(), true)
        .map_err(|_| MaterializeError::Unreadable)?;
    let present = conversation_ids(&source)?;
    if !present
        .iter()
        .any(|id| fenced.iter().any(|item| item == id))
    {
        return Ok(None);
    }
    let mut dest = Connection::open_in_memory().map_err(|_| MaterializeError::Unreadable)?;
    {
        let backup = Backup::new(&source, &mut dest).map_err(|_| MaterializeError::Unreadable)?;
        if !matches!(
            backup.step(-1).map_err(|_| MaterializeError::Unreadable)?,
            StepResult::Done
        ) {
            return Err(MaterializeError::Unreadable);
        }
    }
    for id in fenced {
        dest.execute(
            "DELETE FROM event_records WHERE stream_key IN (
                SELECT stream_key FROM event_streams WHERE public_id = ?1
             )",
            [id],
        )
        .map_err(|_| MaterializeError::Unreadable)?;
        dest.execute("DELETE FROM event_streams WHERE public_id = ?1", [id])
            .map_err(|_| MaterializeError::Unreadable)?;
        dest.execute("DELETE FROM event_stream_names WHERE public_id = ?1", [id])
            .map_err(|_| MaterializeError::Unreadable)?;
    }
    dest.execute_batch("VACUUM")
        .map_err(|_| MaterializeError::Unreadable)?;
    let left = conversation_ids(&dest)?;
    if left.iter().any(|id| fenced.iter().any(|item| item == id)) {
        return Err(MaterializeError::Unreadable);
    }
    serialized_image(dest)
        .map_err(|_| MaterializeError::Unreadable)
        .map(Some)
}

fn conversation_ids(connection: &Connection) -> Result<Vec<String>, MaterializeError> {
    let mut statement = connection
        .prepare(
            "SELECT public_id FROM event_stream_names
             UNION
             SELECT public_id FROM event_streams",
        )
        .map_err(|_| MaterializeError::Unreadable)?;
    let rows = statement
        .query_map([], |row| row.get(0))
        .map_err(|_| MaterializeError::Unreadable)?;
    rows.collect::<Result<Vec<String>, _>>()
        .map_err(|_| MaterializeError::Unreadable)
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
    use nessa_local_database::rusqlite::types::ValueRef;

    #[cfg(target_os = "linux")]
    #[test]
    fn a_sparse_held_session_is_inspected_under_a_one_gib_address_limit() {
        const CHILD: &str = "NESSA_HELD_IMAGE_LIMIT_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","backup::infrastructure::snapshot::tests::a_sparse_held_session_is_inspected_under_a_one_gib_address_limit","--test-threads=1","--nocapture"])
                .env(CHILD,"1").status().unwrap();
            assert!(
                status.success(),
                "bounded held-file inspection child failed: {status}"
            );
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(SESSION_SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO event_streams VALUES (1,'conv',?1,?2,?3,0)",
                nessa_local_database::rusqlite::params![
                    vec![1u8; 16],
                    0u64.to_be_bytes().as_slice(),
                    1u64.to_be_bytes().as_slice()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO event_stream_names VALUES ('conv',?1,1)",
                [vec![1u8; 16]],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO event_records VALUES (1,?1,'event','schema',1,?2)",
                nessa_local_database::rusqlite::params![
                    1u64.to_be_bytes().as_slice(),
                    b"bounded payload".as_slice()
                ],
            )
            .unwrap();
        drop(connection);
        let expected = inspect_conversation(&path, "conv").unwrap();
        let mut file = File::options().read(true).write(true).open(&path).unwrap();
        file.set_len(4 * 1024 * 1024 * 1024).unwrap();
        std::fs::remove_file(&path).unwrap();
        let limit = libc::rlimit {
            rlim_cur: 1024 * 1024 * 1024,
            rlim_max: 1024 * 1024 * 1024,
        };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) }, 0);
        assert_eq!(inspect_open_file(&mut file, "conv").unwrap(), expected);
        assert!(!path.exists());
    }

    fn record_scan_fixture() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SESSION_SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO event_records VALUES (1, ?1, 'event', 'schema', 1, zeroblob(0))",
                [1u64.to_be_bytes().as_slice()],
            )
            .unwrap();
        connection
    }

    #[test]
    fn a_record_scan_visits_before_reading_later_metadata() {
        let connection = record_scan_fixture();
        connection
            .execute(
                "INSERT INTO event_records VALUES (1, ?1, 'event', ?2, 1, zeroblob(0))",
                nessa_local_database::rusqlite::params![
                    2u64.to_be_bytes().as_slice(),
                    "s".repeat(STREAM_METADATA_LIMIT as usize + 1)
                ],
            )
            .unwrap();
        let mut visited = 0;
        let result = visit_digest_records(&connection, 1, |_| {
            visited += 1;
            Err(())
        });
        assert!(result.is_err());
        assert_eq!(
            visited, 1,
            "a later invalid head must not be decoded eagerly"
        );
    }

    #[test]
    fn record_metadata_is_bounded_in_bytes_before_delivery() {
        for column in ["schema_id", "event_id"] {
            let connection = record_scan_fixture();
            let sql = format!("UPDATE event_records SET {column}=?1");
            let boundary = "é".repeat(STREAM_METADATA_LIMIT as usize / 2);
            connection.execute(&sql, [&boundary]).unwrap();
            let mut visited = 0;
            visit_digest_records(&connection, 1, |_| {
                visited += 1;
                Ok::<(), ()>(())
            })
            .unwrap();
            assert_eq!(visited, 1);
            connection.execute(&sql, [format!("{boundary}x")]).unwrap();
            let result = visit_digest_records(&connection, 1, |_| {
                visited += 1;
                Ok::<(), ()>(())
            });
            assert!(result.is_err());
            assert_eq!(visited, 1, "oversized metadata must not reach the visitor");
        }
    }

    #[test]
    fn invalid_ordering_keys_are_refused_before_the_ordered_scan() {
        let connection = record_scan_fixture();
        connection
            .execute_batch("UPDATE event_records SET offset=zeroblob(1048576)")
            .unwrap();
        let mut visited = false;
        let result = visit_digest_records(&connection, 1, |_| {
            visited = true;
            Ok::<(), ()>(())
        });
        assert!(result.is_err());
        assert!(!visited);
    }

    #[test]
    fn a_non_integer_schema_version_is_not_delivered_as_metadata() {
        let connection = record_scan_fixture();
        connection
            .execute_batch("UPDATE event_records SET schema_version='x'||hex(zeroblob(1048576))")
            .unwrap();
        // Observe SQLite's result before Rust's i64 decoder could refuse it:
        // the oversized text is excluded by the query itself.
        let mut statement = connection.prepare(RECORD_HEAD_SQL).unwrap();
        let mut rows = statement.query([1, STREAM_METADATA_LIMIT]).unwrap();
        assert!(matches!(
            rows.next().unwrap().unwrap().get_ref(3).unwrap(),
            ValueRef::Null
        ));
        drop(rows);
        drop(statement);
        let mut visited = false;
        assert!(visit_digest_records(&connection, 1, |_| {
            visited = true;
            Ok::<(), ()>(())
        })
        .is_err());
        assert!(!visited);
    }

    #[test]
    fn a_record_scan_refuses_a_view_in_place_of_stored_columns() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE VIEW event_records AS SELECT 1 stream_key, zeroblob(8) offset,
             'event' event_id, 'schema' schema_id, 1 schema_version, zeroblob(0) payload;",
            )
            .unwrap();
        let result = visit_digest_records(&connection, 1, |_| Ok::<(), ()>(()));
        assert!(result.is_err());
    }

    #[test]
    fn a_record_scan_refuses_generated_metadata_before_evaluation() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE event_records(stream_key INTEGER, offset BLOB,
             event_id TEXT, schema_version INTEGER, payload BLOB,
             schema_id TEXT GENERATED ALWAYS AS (hex(zeroblob(1048576))) VIRTUAL);
             INSERT INTO event_records(stream_key, offset, event_id, schema_version, payload)
             VALUES (1, zeroblob(8), 'event', 1, zeroblob(0));",
            )
            .unwrap();
        assert!(ordinary_stream_table(&connection, "event_records").is_err());
        assert!(visit_digest_records(&connection, 1, |_| Ok::<(), ()>(())).is_err());
    }

    #[test]
    fn a_record_scan_refuses_a_shadowed_physical_rowid() {
        for alias in ["rowid", "_rowid_", "oid"] {
            let connection = record_scan_fixture();
            connection
                .execute_batch(&format!(
                    "ALTER TABLE event_records ADD COLUMN {alias} INTEGER DEFAULT 99"
                ))
                .unwrap();
            assert!(ordinary_stream_table(&connection, "event_records").is_err());
            let mut visited = false;
            assert!(visit_digest_records(&connection, 1, |_| {
                visited = true;
                Ok::<(), ()>(())
            })
            .is_err());
            assert!(!visited);
        }
    }

    #[test]
    fn copied_record_order_uses_an_index_and_delivers_many_small_records() {
        let mut connection = record_scan_fixture();
        let transaction = connection.transaction().unwrap();
        for offset in 2u64..10_002 {
            transaction
                .execute(
                    "INSERT INTO event_records VALUES (1, ?1, 'event', 'schema', 1, zeroblob(0))",
                    [offset.to_be_bytes().as_slice()],
                )
                .unwrap();
        }
        transaction.commit().unwrap();
        let mut previous = 0u64;
        visit_digest_records(&connection, 1, |record| {
            let offset = u64::from_be_bytes(record.offset.try_into().unwrap());
            assert_eq!(offset, previous + 1);
            previous = offset;
            Ok::<(), ()>(())
        })
        .unwrap();
        assert_eq!(previous, 10_001);
        // Query-plan details are test evidence only, not a production decision.
        let plan: String = connection.query_row(
            "EXPLAIN QUERY PLAN SELECT rowid, offset FROM event_records WHERE stream_key=1 ORDER BY offset",
            [], |row| row.get(3),
        ).unwrap();
        assert!(plan.contains("event_records_stream_offset"), "{plan}");
    }

    #[test]
    fn non_integer_stream_keys_are_refused_before_join_comparison() {
        let connection = Connection::open_in_memory().unwrap();
        // No affinity on these malformed stored keys: INTEGER PRIMARY KEY in
        // the supported schema would refuse the fixture at insertion.
        connection.execute_batch(
            "CREATE TABLE event_stream_names(public_id TEXT, active_stream_key);
             CREATE TABLE event_streams(stream_key, incarnation BLOB, floor BLOB, tail BLOB);
             INSERT INTO event_stream_names VALUES ('conv', 'x'||hex(zeroblob(524288)));
             INSERT INTO event_streams VALUES ('x'||hex(zeroblob(524288)), zeroblob(16), zeroblob(8), zeroblob(8));"
        ).unwrap();
        // A malformed active name remains visible as an unmatched head, without
        // comparing or projecting its oversized text key. It cannot disappear
        // from an otherwise successful deferred inventory.
        let key_type: String = connection
            .query_row(
                &format!("SELECT typeof(s.stream_key) {STREAM_SOURCE_SQL}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(key_type, "null");
        assert_eq!(
            one_stream(&connection, "conv").unwrap_err(),
            MaterializeError::Unreadable
        );
        assert!(deferred_streams(&connection).is_err());
        // Supported integer keys remain accepted by the same source query.
        connection
            .execute_batch(
                "UPDATE event_stream_names SET active_stream_key=7;
             UPDATE event_streams SET stream_key=7;",
            )
            .unwrap();
        let key_type: String = connection
            .query_row(
                &format!("SELECT typeof(s.stream_key) {STREAM_SOURCE_SQL}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(key_type, "integer");
        // An orphan active key is corruption; retired and absent names are
        // legitimate missing conversations, not malformed active heads.
        connection
            .execute_batch("UPDATE event_stream_names SET active_stream_key=8")
            .unwrap();
        assert_eq!(
            one_stream(&connection, "conv").unwrap_err(),
            MaterializeError::Unreadable
        );
        assert!(deferred_streams(&connection).is_err());
        connection
            .execute_batch("UPDATE event_stream_names SET active_stream_key=NULL")
            .unwrap();
        assert_eq!(
            one_stream(&connection, "conv").unwrap_err(),
            MaterializeError::Missing
        );
        assert!(deferred_streams(&connection).unwrap().is_empty());
        assert_eq!(
            one_stream(&connection, "absent").unwrap_err(),
            MaterializeError::Missing
        );
    }

    #[test]
    fn oversized_stream_heads_are_refused_before_copying() {
        for column in ["incarnation", "floor", "tail"] {
            let connection = record_scan_fixture();
            connection.execute_batch(
                "INSERT INTO event_streams VALUES (1, 'conv', zeroblob(16), zeroblob(8), zeroblob(8), 0);
                 INSERT INTO event_stream_names VALUES ('conv', zeroblob(16), 1);"
            ).unwrap();
            connection
                .execute_batch(&format!(
                    "UPDATE event_streams SET {column}=zeroblob(1048576)"
                ))
                .unwrap();
            assert_eq!(
                one_stream(&connection, "conv").unwrap_err(),
                MaterializeError::Unreadable
            );
        }
    }

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
