//! One `nessa env serve` per data directory at a time: the lock is
//! `environment/serve.lock`, held for the process's life. A gateway that
//! reconnects while the previous serving process is still ending its leases
//! waits for it, within a bound, rather than being told the host is busy.
use std::{
    fs::{File, TryLockError},
    io,
    path::Path,
    time::{Duration, Instant},
};

/// The held lock; released when dropped, or when the process ends.
pub(crate) struct ServeLock {
    _file: File,
}

/// Why the lock was not taken.
#[derive(Debug)]
pub(crate) enum ServeLockError {
    /// Another serving process still held it after the wait.
    Busy,
    /// The lock file could not be used.
    Io(io::Error),
}

impl ServeLock {
    /// Take the lock at `path`, waiting at most `wait` for another holder.
    ///
    /// # Errors
    /// [`ServeLockError::Busy`] past the wait; [`ServeLockError::Io`] when
    /// the file could not be opened privately.
    pub(crate) async fn acquire(path: &Path, wait: Duration) -> Result<Self, ServeLockError> {
        let file = nessa_local_storage::open(path, nessa_local_storage::OpenMode::OpenOrCreate)
            .map_err(ServeLockError::Io)?;
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(TryLockError::WouldBlock) => return Err(ServeLockError::Busy),
                Err(TryLockError::Error(error)) => return Err(ServeLockError::Io(error)),
            }
        }
    }
}
