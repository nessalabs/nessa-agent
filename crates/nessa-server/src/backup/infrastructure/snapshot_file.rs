//! Read-only SQLite access to a retained file descriptor.
//!
//! The private VFS reads pages with positional I/O. It neither reopens a path
//! nor allocates an image-sized buffer. Registration lives only as long as its
//! connection; the connection is closed before the VFS and retained file drop.

use std::ffi::{c_char, c_int, c_void, CString};
use std::fs::File;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use nessa_local_database::rusqlite::{ffi, Connection, OpenFlags};

static NEXT_VFS: AtomicU64 = AtomicU64::new(1);

struct Source {
    file: File,
    base: *mut ffi::sqlite3_vfs,
    writable: bool,
}

struct Registration {
    vfs: Box<ffi::sqlite3_vfs>,
    _source: Box<Source>,
    _name: CString,
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SQLite has already closed the connection and its file methods.
        unsafe { ffi::sqlite3_vfs_unregister(&mut *self.vfs) };
    }
}

pub(super) struct HeldDatabase {
    connection: Connection,
    _registration: Registration,
}

impl HeldDatabase {
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn open(file: &File) -> Result<Self, ()> {
        Self::with_mode(file, false)
    }

    pub fn writable(file: &File) -> Result<Self, ()> {
        Self::with_mode(file, true)
    }

    fn with_mode(file: &File, writable: bool) -> Result<Self, ()> {
        let name = CString::new(format!(
            "nessa-held-{}",
            NEXT_VFS.fetch_add(1, Ordering::Relaxed)
        ))
        .map_err(|_| ())?;
        let base = unsafe { ffi::sqlite3_vfs_find(ptr::null()) };
        if base.is_null() {
            return Err(());
        }
        let mut source = Box::new(Source {
            file: file.try_clone().map_err(|_| ())?,
            base,
            writable,
        });
        // Version one deliberately excludes mmap and shared-memory methods.
        let mut vfs: Box<ffi::sqlite3_vfs> = Box::new(unsafe { std::mem::zeroed() });
        vfs.iVersion = 1;
        vfs.szOsFile = std::mem::size_of::<HeldFile>() as c_int;
        vfs.mxPathname = 32;
        vfs.zName = name.as_ptr();
        vfs.pAppData = (&mut *source as *mut Source).cast();
        vfs.xOpen = Some(open_file);
        vfs.xDelete = Some(delete_file);
        vfs.xAccess = Some(access);
        vfs.xFullPathname = Some(full_path);
        vfs.xRandomness = Some(randomness);
        vfs.xSleep = Some(sleep);
        vfs.xCurrentTime = Some(current_time);
        if unsafe { ffi::sqlite3_vfs_register(&mut *vfs, 0) } != ffi::SQLITE_OK {
            return Err(());
        }
        let registration = Registration {
            vfs,
            _source: source,
            _name: name,
        };
        let connection = Connection::open_with_flags_and_vfs(
            "held",
            (if writable {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            } else {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            }) | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            registration._name.to_str().map_err(|_| ())?,
        )
        .map_err(|_| ())?;
        connection
            .execute_batch("PRAGMA cache_size=-1024; PRAGMA mmap_size=0; PRAGMA temp_store=FILE;")
            .map_err(|_| ())?;
        if writable {
            connection
                .execute_batch("PRAGMA journal_mode=OFF;")
                .map_err(|_| ())?;
        }
        Ok(Self {
            connection,
            _registration: registration,
        })
    }
}

#[repr(C)]
struct HeldFile {
    base: ffi::sqlite3_file,
    file: *mut File,
    writable: bool,
}

static METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(close_file),
    xRead: Some(read_file),
    xWrite: Some(write_file),
    xTruncate: Some(truncate),
    xSync: Some(sync),
    xFileSize: Some(file_size),
    xLock: Some(lock),
    xUnlock: Some(lock),
    xCheckReservedLock: Some(reserved),
    xFileControl: Some(control),
    xSectorSize: Some(sector),
    xDeviceCharacteristics: Some(characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};

// SQLite supplies live VFS/file pointers and correctly sized callback buffers.
// The VFS accepts only the one read-only main database. No callback unwinds.
unsafe extern "C" fn open_file(
    vfs: *mut ffi::sqlite3_vfs,
    _: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    let source = &*((*vfs).pAppData.cast::<Source>());
    if flags & ffi::SQLITE_OPEN_MAIN_DB == 0
        || (!source.writable && flags & ffi::SQLITE_OPEN_READWRITE != 0)
    {
        return ffi::SQLITE_CANTOPEN;
    }
    let opened = match source.file.try_clone() {
        Ok(file) => file,
        Err(_) => return ffi::SQLITE_CANTOPEN,
    };
    ptr::write(
        file.cast::<HeldFile>(),
        HeldFile {
            base: ffi::sqlite3_file { pMethods: &METHODS },
            file: Box::into_raw(Box::new(opened)),
            writable: source.writable,
        },
    );
    if !out.is_null() {
        *out = if source.writable {
            ffi::SQLITE_OPEN_READWRITE
        } else {
            ffi::SQLITE_OPEN_READONLY
        };
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn close_file(file: *mut ffi::sqlite3_file) -> c_int {
    let held = &mut *file.cast::<HeldFile>();
    drop(Box::from_raw(held.file));
    held.file = ptr::null_mut();
    held.base.pMethods = ptr::null();
    ffi::SQLITE_OK
}
unsafe extern "C" fn read_file(
    file: *mut ffi::sqlite3_file,
    output: *mut c_void,
    amount: c_int,
    offset: i64,
) -> c_int {
    if amount < 0 || offset < 0 {
        return ffi::SQLITE_IOERR_READ;
    }
    let file = &*(*file.cast::<HeldFile>()).file;
    let buffer = std::slice::from_raw_parts_mut(output.cast::<u8>(), amount as usize);
    let mut read = 0;
    while read < buffer.len() {
        match read_at(file, &mut buffer[read..], offset as u64 + read as u64) {
            Ok(0) => {
                buffer[read..].fill(0);
                return ffi::SQLITE_IOERR_SHORT_READ;
            }
            Ok(count) => read += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return ffi::SQLITE_IOERR_READ,
        }
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn file_size(file: *mut ffi::sqlite3_file, size: *mut i64) -> c_int {
    match (&*(*file.cast::<HeldFile>()).file)
        .metadata()
        .and_then(|m| i64::try_from(m.len()).map_err(std::io::Error::other))
    {
        Ok(len) => {
            *size = len;
            ffi::SQLITE_OK
        }
        Err(_) => ffi::SQLITE_IOERR_FSTAT,
    }
}
unsafe extern "C" fn write_file(
    file: *mut ffi::sqlite3_file,
    input: *const c_void,
    amount: c_int,
    offset: i64,
) -> c_int {
    let held = &*file.cast::<HeldFile>();
    if !held.writable {
        return ffi::SQLITE_READONLY;
    }
    if amount < 0 || offset < 0 {
        return ffi::SQLITE_IOERR_WRITE;
    }
    let buffer = std::slice::from_raw_parts(input.cast::<u8>(), amount as usize);
    let mut written = 0;
    while written < buffer.len() {
        match write_at(
            &*held.file,
            &buffer[written..],
            offset as u64 + written as u64,
        ) {
            Ok(0) => return ffi::SQLITE_IOERR_WRITE,
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return ffi::SQLITE_IOERR_WRITE,
        }
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn truncate(file: *mut ffi::sqlite3_file, len: i64) -> c_int {
    let held = &*file.cast::<HeldFile>();
    if !held.writable {
        return ffi::SQLITE_READONLY;
    }
    if len < 0 || (&*held.file).set_len(len as u64).is_err() {
        return ffi::SQLITE_IOERR_TRUNCATE;
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn sync(file: *mut ffi::sqlite3_file, _: c_int) -> c_int {
    if (&*(*file.cast::<HeldFile>()).file).sync_all().is_err() {
        ffi::SQLITE_IOERR_FSYNC
    } else {
        ffi::SQLITE_OK
    }
}
unsafe extern "C" fn lock(_: *mut ffi::sqlite3_file, _: c_int) -> c_int {
    ffi::SQLITE_OK
}
unsafe extern "C" fn reserved(_: *mut ffi::sqlite3_file, output: *mut c_int) -> c_int {
    *output = 0;
    ffi::SQLITE_OK
}
unsafe extern "C" fn control(_: *mut ffi::sqlite3_file, _: c_int, _: *mut c_void) -> c_int {
    ffi::SQLITE_NOTFOUND
}
unsafe extern "C" fn sector(_: *mut ffi::sqlite3_file) -> c_int {
    4096
}
unsafe extern "C" fn characteristics(file: *mut ffi::sqlite3_file) -> c_int {
    if (*file.cast::<HeldFile>()).writable {
        0
    } else {
        ffi::SQLITE_IOCAP_IMMUTABLE
    }
}
unsafe extern "C" fn delete_file(_: *mut ffi::sqlite3_vfs, _: *const c_char, _: c_int) -> c_int {
    ffi::SQLITE_READONLY
}
unsafe extern "C" fn access(
    _: *mut ffi::sqlite3_vfs,
    _: *const c_char,
    _: c_int,
    output: *mut c_int,
) -> c_int {
    *output = 0;
    ffi::SQLITE_OK
}
unsafe extern "C" fn full_path(
    _: *mut ffi::sqlite3_vfs,
    _: *const c_char,
    len: c_int,
    output: *mut c_char,
) -> c_int {
    if len < 5 {
        return ffi::SQLITE_CANTOPEN;
    }
    ptr::copy_nonoverlapping(c"held".as_ptr(), output, 5);
    ffi::SQLITE_OK
}
unsafe extern "C" fn randomness(
    vfs: *mut ffi::sqlite3_vfs,
    len: c_int,
    output: *mut c_char,
) -> c_int {
    let base = (*(*vfs).pAppData.cast::<Source>()).base;
    match (*base).xRandomness {
        Some(call) => call(base, len, output),
        None => 0,
    }
}
unsafe extern "C" fn sleep(vfs: *mut ffi::sqlite3_vfs, micros: c_int) -> c_int {
    let base = (*(*vfs).pAppData.cast::<Source>()).base;
    match (*base).xSleep {
        Some(call) => call(base, micros),
        None => 0,
    }
}
unsafe extern "C" fn current_time(vfs: *mut ffi::sqlite3_vfs, output: *mut f64) -> c_int {
    let base = (*(*vfs).pAppData.cast::<Source>()).base;
    match (*base).xCurrentTime {
        Some(call) => call(base, output),
        None => ffi::SQLITE_ERROR,
    }
}

#[cfg(unix)]
#[test]
fn an_unlinked_sparse_image_is_read_by_pages_and_stays_read_only() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE note(body); INSERT INTO note VALUES ('sealed');")
        .unwrap();
    drop(db);
    let file = File::options().read(true).write(true).open(&path).unwrap();
    file.set_len(4 * 1024 * 1024 * 1024).unwrap();
    std::fs::remove_file(&path).unwrap();
    let db = HeldDatabase::open(&file).unwrap();
    assert_eq!(
        db.connection
            .query_row("SELECT body FROM note", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "sealed"
    );
    assert!(db
        .connection
        .execute("UPDATE note SET body='changed'", [])
        .is_err());
    assert!(!path.exists());
}

fn read_at(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<usize> {
    #[cfg(unix)]
    {
        file.read_at(bytes, offset)
    }
    #[cfg(windows)]
    {
        file.seek_read(bytes, offset)
    }
}
fn write_at(file: &File, bytes: &[u8], offset: u64) -> std::io::Result<usize> {
    #[cfg(unix)]
    {
        file.write_at(bytes, offset)
    }
    #[cfg(windows)]
    {
        file.seek_write(bytes, offset)
    }
}

#[test]
fn an_unindexed_sort_that_needs_a_temp_file_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sorter");
    let mut source = Connection::open(&path).unwrap();
    source
        .execute_batch("CREATE TABLE sorter(value TEXT); PRAGMA journal_mode=OFF;")
        .unwrap();
    let transaction = source.transaction().unwrap();
    {
        let mut insert = transaction
            .prepare("INSERT INTO sorter VALUES (?1)")
            .unwrap();
        for index in 0..4096 {
            insert
                .execute([format!("{index:08}{}", "x".repeat(4096))])
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    drop(source);
    let file = File::open(&path).unwrap();
    let held = HeldDatabase::open(&file).unwrap();
    let mut sorted = held
        .connection()
        .prepare("SELECT value FROM sorter ORDER BY value DESC")
        .unwrap();
    let mut rows = sorted.query([]).unwrap();
    assert!(
        rows.next().is_err(),
        "a spilling sort must not be satisfied by unbounded memory or an unrelated pathname"
    );
    let value: String = held
        .connection()
        .query_row("SELECT value FROM sorter LIMIT 1", [], |row| row.get(0))
        .unwrap();
    assert!(value.starts_with("00000000"));
}
