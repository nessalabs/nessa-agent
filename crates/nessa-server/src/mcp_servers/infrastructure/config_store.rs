//! The stored servers in `config.json`: read, and rewritten as a whole under
//! `config.json.lock`, with only the `agents.mcpServers` block changed.
//!
//! ```text
//! ConfigJsonStore ──read / publish / try_lock──▶ ConfigFiles (OsConfigFiles: the file, its lock)
//!                 ──check──────────────────────▶ the runtime configuration's own parse and bound
//!                 ──now / sleep_until──────────▶ Clock (the lock's bounded wait)
//!                 ──stored_revision────────────▶ ConfigurationKey (this process's, the stand-ins' too)
//! ```
//!
//! Arrows are calls. A configuration that does not pass the check, before or
//! after the edit, is refused and never repaired
//! (`s8_a_configuration_that_does_not_parse_is_refused_and_never_repaired`).
//! The check is composition's
//! (`RuntimeConfig`), so this holds no second reading of the file.
//!
//! A write re-serialises the whole file from its parsed value: the gateway
//! owns `config.json`, so after a write its layout and key order are the
//! gateway's, and everything outside `agents.mcpServers` keeps its value,
//! not its spelling (`a_save_is_published_then_replaces_the_live_set_and_is_audited_both_sides`).
use super::stored_servers::{block, parse_block, revision};
use crate::mcp_servers::application::{
    McpServerStore, StoreError, StoreFuture, StoreLock, StoredServers,
};
use crate::mcp_servers::domain::{ConfigurationKey, ConfiguredMcpServer};
use nessa_sdk::infrastructure::clock::Clock;
use serde_json::{Map, Value};
use std::{io, sync::Arc, time::Duration};

/// How long a change waits for `config.json.lock` before it is refused
/// [`StoreError::Busy`].
pub const LOCK_WAIT: Duration = Duration::from_secs(2);
/// How often a waiting change tries the lock again.
const LOCK_RETRY: Duration = Duration::from_millis(20);

/// The file system under the store: the configuration file and its lock.
pub trait ConfigFiles: Send + Sync {
    /// The file's bytes, at most `limit + 1` of them; `None` when there is no
    /// file.
    fn read(&self, limit: usize) -> io::Result<Option<Vec<u8>>>;
    /// Replace the file with `bytes` in one step, private to this user.
    fn publish(&self, bytes: &[u8]) -> io::Result<()>;
    /// The lock, or `None` while another holder has it.
    fn try_lock(&self) -> io::Result<Option<StoreLock>>;
}

/// Whether some bytes parse as a configuration this gateway starts with.
pub type ConfigParse = Box<dyn Fn(&[u8]) -> bool + Send + Sync>;

/// The runtime configuration's own bound and parse of a whole file.
pub struct ConfigCheck {
    /// The most bytes a configuration may have.
    pub limit: usize,
    /// Whether `bytes` parse as a configuration this gateway starts with.
    pub parses: ConfigParse,
}

/// `config.json`'s `agents.mcpServers`, as [`McpServerStore`].
pub struct ConfigJsonStore {
    files: Arc<dyn ConfigFiles>,
    check: ConfigCheck,
    /// The `agents` block a write starts from when the file has none: the
    /// catalog and workspace this gateway runs with, which a desktop gateway
    /// composed without one.
    agents: Map<String, Value>,
    /// What the lock's bounded wait is measured on.
    clock: Arc<dyn Clock>,
    /// What each revision is keyed with: the process's one key, which the
    /// stand-ins' digests are keyed with too.
    key: ConfigurationKey,
}

impl ConfigJsonStore {
    pub fn new(
        files: Arc<dyn ConfigFiles>,
        check: ConfigCheck,
        agents: Map<String, Value>,
        clock: Arc<dyn Clock>,
        key: ConfigurationKey,
    ) -> Self {
        Self {
            files,
            check,
            agents,
            clock,
            key,
        }
    }

    /// The file as a JSON object, after the runtime configuration's check;
    /// an object with nothing in it when there is no file.
    fn document(&self) -> Result<Map<String, Value>, StoreError> {
        let Some(bytes) = self
            .files
            .read(self.check.limit)
            .map_err(|_| StoreError::Unavailable)?
        else {
            return Ok(Map::new());
        };
        self.checked(&bytes)?;
        match serde_json::from_slice(&bytes) {
            Ok(Value::Object(document)) => Ok(document),
            _ => Err(StoreError::ConfigInvalid),
        }
    }

    fn checked(&self, bytes: &[u8]) -> Result<(), StoreError> {
        if bytes.len() > self.check.limit {
            return Err(StoreError::ConfigTooLarge);
        }
        if !(self.check.parses)(bytes) {
            return Err(StoreError::ConfigInvalid);
        }
        Ok(())
    }
}

/// The stored block in `document`, when there is one.
fn stored_block(document: &Map<String, Value>) -> Option<&Value> {
    document.get("agents")?.as_object()?.get("mcpServers")
}

impl McpServerStore for ConfigJsonStore {
    fn lock(&self) -> StoreFuture<'_, StoreLock> {
        Box::pin(async move {
            let deadline = self.clock.now() + LOCK_WAIT;
            loop {
                let files = self.files.clone();
                match tokio::task::spawn_blocking(move || files.try_lock()).await {
                    Ok(Ok(Some(lock))) => return Ok(lock),
                    Ok(Ok(None)) if self.clock.now() >= deadline => return Err(StoreError::Busy),
                    Ok(Ok(None)) => {
                        let retry = self.clock.now() + LOCK_RETRY;
                        self.clock.sleep_until(retry.min(deadline)).await;
                    }
                    Ok(Err(_)) | Err(_) => return Err(StoreError::Unavailable),
                }
            }
        })
    }

    fn read(&self) -> Result<StoredServers, StoreError> {
        let document = self.document()?;
        let stored = stored_block(&document);
        let servers = match stored {
            Some(block) => parse_block(block).ok_or(StoreError::ConfigInvalid)?,
            None => Vec::new(),
        };
        Ok(StoredServers {
            revision: revision(&self.key, stored),
            servers,
        })
    }

    fn write(&self, expected: &str, servers: &[ConfiguredMcpServer]) -> Result<String, StoreError> {
        let mut document = self.document()?;
        // What is stored now is what the edit was made to, or the edit is
        // stale: something wrote outside the lock since the read.
        let current = revision(&self.key, stored_block(&document));
        if current != expected {
            return Err(StoreError::RevisionConflict { revision: current });
        }
        let written = block(servers).ok_or(StoreError::ConfigInvalid)?;
        let new_revision = revision(&self.key, Some(&written));
        let agents = document
            .entry("agents")
            .or_insert_with(|| Value::Object(self.agents.clone()));
        let Value::Object(agents) = agents else {
            return Err(StoreError::ConfigInvalid);
        };
        agents.insert("mcpServers".into(), written);
        let mut bytes = serde_json::to_vec_pretty(&Value::Object(document))
            .map_err(|_| StoreError::ConfigInvalid)?;
        bytes.push(b'\n');
        self.checked(&bytes)?;
        self.files
            .publish(&bytes)
            .map_err(|_| StoreError::Unavailable)?;
        Ok(new_revision)
    }
}

/// `config.json` and `config.json.lock` beside it, on this machine.
#[cfg(unix)]
pub struct OsConfigFiles {
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl OsConfigFiles {
    /// The configuration at `path`, locked by `<path>.lock`.
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    fn lock_path(&self) -> std::path::PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(".lock");
        name.into()
    }

    fn directory(&self) -> io::Result<&std::path::Path> {
        self.path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no directory"))
    }
}

#[cfg(unix)]
impl ConfigFiles for OsConfigFiles {
    fn read(&self, limit: usize) -> io::Result<Option<Vec<u8>>> {
        use std::io::Read;
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(io::Error::other("config.json must be a regular file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
        let file = nessa_local_storage::open(&self.path, nessa_local_storage::OpenMode::Read)?;
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    fn publish(&self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let directory = self.directory()?;
        let mut file = nessa_local_storage::PrivateTempFile::new_in(directory)?;
        file.as_file_mut().write_all(bytes)?;
        file.as_file().sync_all()?;
        file.persist(&self.path)?;
        nessa_local_storage::sync_directory(directory)
    }

    fn try_lock(&self) -> io::Result<Option<StoreLock>> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.lock_path())?;
        // SAFETY: flock has no memory preconditions; the descriptor is open.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::EWOULDBLOCK) => Ok(None),
                _ => Err(error),
            };
        }
        // Released when the descriptor is closed, as the lock is dropped.
        Ok(Some(Box::new(lock)))
    }
}
