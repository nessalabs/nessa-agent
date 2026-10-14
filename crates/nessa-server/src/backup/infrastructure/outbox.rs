//! The device outbox and the environment outbox, read while admission is held.
//!
//! ```text
//! pin (admission still held)
//!   directory absent ──▶ watermark absent, no rows
//!   not a directory, a symlink, a short read, a file past the artifact
//!   bound, or a name this cut cannot keep ──▶ fail the source
//!   each regular file ──▶ open without following a link, read the
//!   length just recorded, keep those bytes
//!   watermark = outbox_watermark(identity, length)
//! ```
//!
//! Arrows are the pin's steps. A failure here aborts the cut before deletion
//! fences are appended and before anything is published. Bytes appended
//! after this pin are not part of the watermark or the rows.

use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use nessa_protocol::lease::MAX_ARTIFACT_BYTES;

use super::gateway::content_is_secret;
use crate::backup::application::{CopiedStore, ExportSource, PinnedStore};
use crate::backup::domain::{outbox_watermark, CapturedRecord, Sensitivity, SourceCut, StoreKind};

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
    let entries = fs::read_dir(directory).map_err(|_| ())?;
    for entry in entries {
        let entry = entry.map_err(|_| ())?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|_| ())?;
        if meta.file_type().is_symlink() {
            return Err(());
        }
        if meta.is_dir() {
            collect(root, &path, rows)?;
            continue;
        }
        if !meta.is_file() {
            return Err(());
        }
        rows.push(read_row(root, &path)?);
    }
    Ok(())
}

/// Open `path` without following a final link and read exactly the length
/// of that open file.
fn read_row(root: &Path, path: &Path) -> Result<Row, ()> {
    let identity = relative_identity(root, path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ())?;
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
