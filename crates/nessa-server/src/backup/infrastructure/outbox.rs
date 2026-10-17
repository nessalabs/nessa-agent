//! The device outbox and the environment outbox, read while admission is held.
//!
//! ```text
//! pin (admission still held)
//!   create the lock parent if absent, then outbox.lock shared for the whole read
//!   directory absent ──▶ watermark absent, no rows
//!   not a directory, a symlink, a short read, a file past the artifact
//!   bound, more files or bytes than one lease may keep, or a name this
//!   cut cannot keep ──▶ fail the source
//!   each regular file ──▶ open without following a link, read the
//!   length just recorded, keep those bytes
//!   watermark = outbox_watermark(identity, length)
//!   outbox.lock released
//! ```
//!
//! Arrows are the pin's steps. A failure here aborts the cut before deletion
//! fences are appended and before anything is published. The lock is the
//! sibling `outbox.lock` (`environment/outbox` → `environment/outbox.lock`).
//! `FileOutbox` holds that same file exclusively across a stage, a discard,
//! a lease close, and the sweep when serving starts, so this read does not
//! observe a copy in progress or a row removed halfway through.
//! `an_environment_outbox_cut_waits_for_the_writer_lock`.
//! The files and bytes kept are at most what one lease may keep
//! (`ArtifactBudget::LEASE`); past that the source fails before those
//! bodies are read. `an_outbox_past_the_lease_budget_aborts_the_cut`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use nessa_local_storage::{create_directory, open, OpenMode};
use nessa_protocol::lease::MAX_ARTIFACT_BYTES;

use super::gateway::content_is_secret;
use crate::backup::application::{CopiedStore, ExportSource, PinnedStore};
use crate::backup::domain::{outbox_watermark, CapturedRecord, Sensitivity, SourceCut, StoreKind};
use crate::conversation::application::ArtifactBudget;

#[cfg(test)]
#[path = "../../../tests/backup/outbox_lock.rs"]
mod lock_tests;

/// One outbox directory. `kind` is the device outbox or the environment outbox.
pub(crate) struct OutboxDirectory {
    kind: StoreKind,
    root: PathBuf,
}

impl OutboxDirectory {
    pub(crate) fn new(kind: StoreKind, root: PathBuf) -> Self {
        Self { kind, root }
    }
}

#[async_trait]
impl ExportSource for OutboxDirectory {
    fn kind(&self) -> StoreKind {
        self.kind
    }

    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()> {
        let kind = self.kind;
        let root = self.root.clone();
        let cut = tokio::task::spawn_blocking(move || read_outbox(kind, &root))
            .await
            .map_err(|_| ())??;
        Ok(Box::new(CopiedStore::new(cut)))
    }
}

struct Row {
    identity: String,
    len: u64,
    bytes: Vec<u8>,
}

fn read_outbox(kind: StoreKind, root: &Path) -> Result<SourceCut, ()> {
    let _lock = shared_outbox_lock(root)?;
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return SourceCut::absent(kind).map_err(|_| ());
        }
        Err(_) => return Err(()),
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return Err(()),
        Ok(_) => {}
    }
    let mut rows = Vec::new();
    collect(root, root, &mut rows)?;
    rows.sort_by(|left, right| left.identity.cmp(&right.identity));
    let pairs: Vec<(&str, u64)> = rows
        .iter()
        .map(|row| (row.identity.as_str(), row.len))
        .collect();
    let watermark = outbox_watermark(&pairs).map_err(|_| ())?;
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        let sensitivity = if content_is_secret(&row.bytes) {
            Sensitivity::Secret
        } else {
            Sensitivity::Public
        };
        records
            .push(CapturedRecord::new(kind, row.identity, sensitivity, row.bytes).map_err(|_| ())?);
    }
    SourceCut::new(kind, watermark, records, Vec::new()).map_err(|_| ())
}

fn collect(root: &Path, directory: &Path, rows: &mut Vec<Row>) -> Result<(), ()> {
    let mut pending = Vec::new();
    let mut total = 0u64;
    list_rows(directory, &mut pending, &mut total)?;
    for path in pending {
        rows.push(read_row(root, &path)?);
    }
    Ok(())
}

/// Names and lengths only. A directory past the lease budget fails here,
/// before any body is read.
fn list_rows(directory: &Path, pending: &mut Vec<PathBuf>, total: &mut u64) -> Result<(), ()> {
    let entries = fs::read_dir(directory).map_err(|_| ())?;
    for entry in entries {
        let entry = entry.map_err(|_| ())?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|_| ())?;
        if meta.file_type().is_symlink() {
            return Err(());
        }
        if meta.is_dir() {
            list_rows(&path, pending, total)?;
            continue;
        }
        if !meta.is_file() {
            return Err(());
        }
        count_toward_budget(pending.len(), total, meta.len())?;
        pending.push(path);
    }
    Ok(())
}

/// Count one more file toward the snapshot copied while admission is held.
///
/// The ceiling is the files and bytes one lease may keep. Past either, the
/// body is not read. `an_outbox_past_the_lease_budget_aborts_the_cut`.
fn count_toward_budget(counted: usize, total: &mut u64, len: u64) -> Result<(), ()> {
    let files = usize::try_from(ArtifactBudget::LEASE.files).map_err(|_| ())?;
    if counted >= files {
        return Err(());
    }
    let next = total.checked_add(len).ok_or(())?;
    if next > ArtifactBudget::LEASE.bytes {
        return Err(());
    }
    *total = next;
    Ok(())
}

/// `outbox.lock` beside `root`, shared for as long as the returned file lives.
///
/// Establish the lock even before the first writer creates the outbox.
/// `absent_parent_outbox_cut_excludes_first_writer` covers both orderings.
fn shared_outbox_lock(root: &Path) -> Result<File, ()> {
    let Some(parent) = root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Err(());
    };
    match fs::symlink_metadata(parent) {
        Ok(meta) if meta.is_dir() => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_directory(parent).map_err(|_| ())?;
        }
        _ => return Err(()),
    }
    let file = open(&root.with_extension("lock"), OpenMode::OpenOrCreate).map_err(|_| ())?;
    file.lock_shared().map_err(|_| ())?;
    Ok(file)
}

/// Open `path` without following a final link.
///
/// Unix uses `O_NOFOLLOW`. Windows opens the reparse point, so a symlink is
/// not a regular file and [`read_row`] refuses it.
fn open_nofollow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT. The open names the link, not its target.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "an outbox file is opened without following a link",
        ));
    }
    options.open(path)
}

/// Open `path` without following a final link and read exactly the length
/// of that open file.
fn read_row(root: &Path, path: &Path) -> Result<Row, ()> {
    let identity = relative_identity(root, path)?;
    let file = open_nofollow(path).map_err(|_| ())?;
    let opened = file.metadata().map_err(|_| ())?;
    if !opened.is_file() {
        return Err(());
    }
    let len = opened.len();
    if len > MAX_ARTIFACT_BYTES {
        return Err(());
    }
    let mut bytes = Vec::new();
    file.take(len).read_to_end(&mut bytes).map_err(|_| ())?;
    if bytes.len() as u64 != len {
        return Err(());
    }
    Ok(Row {
        identity,
        len,
        bytes,
    })
}

fn relative_identity(root: &Path, path: &Path) -> Result<String, ()> {
    let relative = path.strip_prefix(root).map_err(|_| ())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or(())?;
                parts.push(part);
            }
            _ => return Err(()),
        }
    }
    if parts.is_empty() {
        return Err(());
    }
    Ok(parts.join("/"))
}
