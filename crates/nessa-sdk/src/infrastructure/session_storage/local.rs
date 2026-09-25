use super::{paths::SessionPaths, snapshot};
use crate::application::agent_execution::sessions::storage::{
    SessionSnapshot, SessionStorage, SessionStorageLease, StorageError, StorageFuture,
};
use crate::domain::agent_execution::sessions::SessionId;
use nessa_local_storage::{self as private, OpenMode};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    fs::{File, TryLockError},
    io::{self, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::SystemTime,
};

/// One private JSONL history and persistent lock file per exact session identity.
/// Filenames use `s-` plus lowercase unpadded base32hex of the identity bytes,
/// followed by `.jsonl` or `.lock`. Distinct letter case in IDs stays distinct on
/// case-insensitive filesystems. Maximum-length IDs produce 213-byte filenames.
/// Each save
/// appends changed records and syncs the file before returning. The directory is
/// also synced, including after creation and recovery of an unacknowledged write.
/// Incomplete final lines are truncated under the lease; malformed complete lines
/// are rejected. Successful saves remain complete checkpoints after recovery.
/// The final lease owner unlocks only after outstanding I/O finishes, even when
/// callers drop their futures or unrelated children inherit file descriptors.
/// Writers must honor the exclusive lease; editing the journal outside that lease
/// while it is live is unsupported. Explicit loads always reread the actual file.
/// Erasing a session removes its journal and syncs the directory under the
/// lease; the empty lock file stays, named only by the identity, because it is
/// what excludes a second writer. [`SessionStorage::open_existing`] opens only
/// a session whose lock file is there, and creates none.
#[derive(Clone)]
pub struct LocalFileStorage {
    root: PathBuf,
}
impl LocalFileStorage {
    /// Creates or verifies `root`, the private directory holding session files.
    ///
    /// Returns a storage error if the directory cannot be created or does not
    /// satisfy private-file ownership and permission checks. Session leases are
    /// acquired separately through [`SessionStorage::open`].
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root = root.into();
        private::create_directory(&root).map_err(io_error)?;
        Ok(Self { root })
    }
}
impl LocalFileStorage {
    /// Lock `id`'s lease file and return its lease. With `create` false a
    /// session with neither a lease file nor a journal is `None` and nothing
    /// is created: every session that was ever opened has a lease file, kept
    /// even when the history is erased. A journal whose lease file is gone is
    /// still a session with history — never taken for one that never was — so
    /// its lease file is made again and the session opened
    /// (`a_journal_whose_lock_is_gone_still_exists`).
    fn acquire(
        root: PathBuf,
        id: SessionId,
        create: bool,
    ) -> Result<Option<Box<dyn SessionStorageLease>>, StorageError> {
        private::verify_directory(&root).map_err(io_error)?;
        let paths = SessionPaths::new(&root, &id);
        let has_history = || match std::fs::symlink_metadata(&paths.journal) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(io_error(error)),
        };
        let mode = if create || has_history()? {
            OpenMode::OpenOrCreate
        } else {
            OpenMode::ReadWrite
        };
        let lock = match private::open(&paths.lock, mode) {
            Ok(lock) => lock,
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        lock.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => StorageError::Busy,
            TryLockError::Error(error) => io_error(error),
        })?;
        Ok(Some(Box::new(LocalStore {
            path: paths.journal,
            root,
            id,
            lease: Arc::new(Lease {
                lock,
                operation: Mutex::new(None),
                #[cfg(test)]
                fault: Mutex::new(None),
            }),
        }) as Box<dyn SessionStorageLease>))
    }
}
impl SessionStorage for LocalFileStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        let root = self.root.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                // Only a lease file that is not there is `None`, and with
                // `create` this call made it.
                Self::acquire(root, id, true)?
                    .ok_or_else(|| StorageError::Io("session lease file was not created".into()))
            })
            .await
            .map_err(task_error)?
        })
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        let root = self.root.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || Self::acquire(root, id, false))
                .await
                .map_err(task_error)?
        })
    }
}
struct Lease {
    lock: File,
    operation: Mutex<Option<Cached>>,
    #[cfg(test)]
    fault: Mutex<Option<SaveFault>>,
}
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveFault {
    PartialWrite,
    FileSync,
    DirectorySync,
}
#[cfg(test)]
fn injected_failure() -> StorageError {
    StorageError::Io("injected journal write failure".into())
}
impl Drop for Lease {
    fn drop(&mut self) {
        // A concurrently forked child can retain this open file description
        // until exec. Closing our descriptor alone would leave its flock held.
        // The final Arc drops only after every outstanding I/O operation ends.
        if let Err(error) = self.lock.unlock() {
            tracing::error!(%error, "session storage writer lease unlock failed");
        }
    }
}
struct LocalStore {
    root: PathBuf,
    path: PathBuf,
    id: SessionId,
    lease: Arc<Lease>,
}
struct Cached {
    snapshot: Option<SessionSnapshot>,
    sequence: u64,
    stamp: FileStamp,
}
#[derive(PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl FileStamp {
    fn read(file: &File) -> Result<Self, StorageError> {
        let metadata = file.metadata().map_err(io_error)?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified().map_err(io_error)?,
            created: metadata.created().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}
impl SessionStorageLease for LocalStore {
    /// A journal that cannot be read is logged with its path, which the
    /// session's identity alone does not give an operator: the file is named
    /// by an encoding of it (`SessionPaths`)
    /// (`a_journal_that_cannot_be_read_is_logged_with_its_path`).
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        let (path, id, lease) = (self.path.clone(), self.id.clone(), self.lease.clone());
        let journal = self.path.clone();
        Box::pin(async move {
            let loaded = tokio::task::spawn_blocking(move || {
                let mut cache = lease
                    .operation
                    .lock()
                    .map_err(|_| StorageError::Io("file storage lock poisoned".into()))?;
                // Always read actual bytes for explicit loads, including externally damaged files.
                *cache = None;
                let mut file = match private::open(&path, OpenMode::ReadWrite) {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(io_error(error)),
                };
                let loaded = recover(&mut file, &id)?;
                let value = loaded.snapshot.clone();
                *cache = Some(loaded);
                Ok(value)
            })
            .await
            .map_err(task_error)
            .and_then(|loaded| loaded);
            if let Err(error) = &loaded {
                tracing::warn!(
                    journal = %journal.display(),
                    %error,
                    "session journal could not be read"
                );
            }
            loaded
        })
    }
    fn save(&self, value: SessionSnapshot) -> StorageFuture<'_, ()> {
        let validation = if value.id != self.id {
            Err(StorageError::IdentityMismatch)
        } else {
            snapshot::validate(&value)
        };
        if let Err(error) = validation {
            value.discard_rejected_errors();
            return Box::pin(async move { Err(error) });
        }
        let (root, path, lease) = (self.root.clone(), self.path.clone(), self.lease.clone());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut cache = lease
                    .operation
                    .lock()
                    .map_err(|_| StorageError::Io("file storage lock poisoned".into()))?;
                private::verify_directory(&root).map_err(io_error)?;
                let mut file = private::open(&path, OpenMode::OpenOrCreate).map_err(io_error)?;
                let stamp = FileStamp::read(&file)?;
                // Taking the cache before any fallible write guarantees that a retry
                // reconciles a complete-but-unacknowledged line or partial tail first.
                let previous = match cache.take() {
                    Some(previous) if previous.stamp == stamp => previous,
                    _ => recover(&mut file, &value.id)?,
                };
                let sequence = previous
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| StorageError::Corrupt("journal sequence exhausted".into()))?;
                let bytes = snapshot::encode(previous.snapshot.as_ref(), &value, sequence)?;
                #[cfg(test)]
                let fault = lease.fault.lock().unwrap().take();
                let saved_sequence = if let Some(bytes) = bytes {
                    file.seek(SeekFrom::End(0)).map_err(io_error)?;
                    #[cfg(test)]
                    if fault == Some(SaveFault::PartialWrite) {
                        file.write_all(&bytes[..bytes.len() / 2])
                            .map_err(io_error)?;
                        return Err(injected_failure());
                    }
                    file.write_all(&bytes).map_err(io_error)?;
                    sequence
                } else {
                    previous.sequence
                };
                // Also sync no-op retries: a preceding attempt may have written a
                // complete line but failed before acknowledging file/directory sync.
                #[cfg(test)]
                if fault == Some(SaveFault::FileSync) {
                    return Err(injected_failure());
                }
                file.sync_all().map_err(io_error)?;
                #[cfg(test)]
                if fault == Some(SaveFault::DirectorySync) {
                    return Err(injected_failure());
                }
                private::sync_directory(&root).map_err(io_error)?;
                *cache = Some(Cached {
                    snapshot: Some(value),
                    sequence: saved_sequence,
                    stamp: FileStamp::read(&file)?,
                });
                Ok(())
            })
            .await
            .map_err(task_error)?
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        let (root, path, lease) = (self.root.clone(), self.path.clone(), self.lease.clone());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut cache = lease
                    .operation
                    .lock()
                    .map_err(|_| StorageError::Io("file storage lock poisoned".into()))?;
                // Forget what was read before any fallible step, so a save
                // after a failed erase reconciles the actual file first.
                *cache = None;
                private::verify_directory(&root).map_err(io_error)?;
                // Only the journal goes. The lock file is the exclusion this
                // lease holds; unlinking it would let the next opener lock a
                // new file while this lease still holds the old one.
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(io_error(error)),
                }
                private::sync_directory(&root).map_err(io_error)
            })
            .await
            .map_err(task_error)?
        })
    }
}
fn recover(file: &mut File, id: &SessionId) -> Result<Cached, StorageError> {
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let loaded = snapshot::read(&mut *file, id)?;
    if loaded.incomplete {
        file.set_len(loaded.committed_bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
    }
    Ok(Cached {
        snapshot: loaded.snapshot,
        sequence: loaded.sequence,
        stamp: FileStamp::read(file)?,
    })
}
fn io_error(error: io::Error) -> StorageError {
    StorageError::Io(error.to_string())
}
fn task_error(error: tokio::task::JoinError) -> StorageError {
    StorageError::Io(error.to_string())
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
