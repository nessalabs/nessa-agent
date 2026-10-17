//! The published bundle: a manifest and one file per public record.
//! Redacted records have no file. On Unix, directories are mode `0700` and
//! files are mode `0600` (`a_published_bundle_is_owner_only`).

use crate::backup::domain::encode_identity as encode;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use nessa_local_storage::{create_directory, open, sync_directory, OpenMode};

use async_trait::async_trait;

use crate::backup::application::{ManifestPublisher, PublishedRecord};
use crate::backup::domain::StoreKind;

/// Writes a bundle under `root`. Identities are percent-encoded, so a
/// record cannot choose a path outside its store directory.
pub struct DirectoryPublisher {
    root: PathBuf,
}

impl DirectoryPublisher {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl ManifestPublisher for DirectoryPublisher {
    async fn put(&self, manifest: &str, records: &[PublishedRecord]) -> Result<(), ()> {
        let root = self.root.clone();
        let manifest = manifest.to_owned();
        let records = records.to_vec();
        tokio::task::spawn_blocking(move || write_bundle(&root, &manifest, &records))
            .await
            .map_err(|_| ())?
    }
}

fn write_bundle(root: &Path, manifest: &str, records: &[PublishedRecord]) -> Result<(), ()> {
    // One directory per attempt. Two publishes of an absent root must not
    // write the same staging path: admission does not cover publish.
    let staging = StagedDir::create(root)?;
    for record in records {
        let path = record_path(staging.path(), record.store, &record.identity)?;
        if let Some(parent) = path.parent() {
            create_directory(parent).map_err(|_| ())?;
        }
        write_synced(&path, &record.bytes)?;
    }
    write_synced(&staging.path().join("manifest.txt"), manifest.as_bytes())?;
    // Directory entries, then the rename, then the parent. Success means the
    // published root is durable.
    sync_tree(staging.path())?;
    if root.exists() {
        return Err(());
    }
    let parent = root.parent().ok_or(())?;
    fs::rename(staging.path(), root).map_err(|_| ())?;
    sync_directory(parent).map_err(|_| ())
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), ()> {
    let mut file = open(path, OpenMode::CreateNew).map_err(|_| ())?;
    file.write_all(bytes).map_err(|_| ())?;
    file.sync_all().map_err(|_| ())
}

fn sync_tree(dir: &Path) -> Result<(), ()> {
    let mut children = Vec::new();
    for entry in fs::read_dir(dir).map_err(|_| ())? {
        let path = entry.map_err(|_| ())?.path();
        if path.is_dir() {
            children.push(path);
        }
    }
    for child in children {
        sync_tree(&child)?;
    }
    sync_directory(dir).map_err(|_| ())
}

/// Removed on drop, including when publish fails. A successful rename moves
/// the directory away, so the drop then finds nothing to remove.
struct StagedDir {
    path: PathBuf,
}

impl StagedDir {
    fn create(root: &Path) -> Result<Self, ()> {
        let path = staging(root)?;
        // The name is this attempt's. A directory already there is a crashed
        // attempt, not another publish in progress.
        let _ = fs::remove_dir_all(&path);
        create_directory(&path).map_err(|_| ())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn staging(root: &Path) -> Result<PathBuf, ()> {
    static ATTEMPT: AtomicU64 = AtomicU64::new(1);
    let name = root.file_name().ok_or(())?;
    let mut staged = name.to_os_string();
    staged.push(format!(
        ".partial-{}-{}",
        std::process::id(),
        ATTEMPT.fetch_add(1, Ordering::Relaxed)
    ));
    Ok(root.with_file_name(staged))
}

fn record_path(root: &Path, store: StoreKind, identity: &str) -> Result<PathBuf, ()> {
    let name = encode(identity);
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(());
    }
    Ok(root.join("records").join(store.as_str()).join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_publish_attempt_has_its_own_staging_directory() {
        let root = Path::new("/tmp/export-cut");
        let first = staging(root).unwrap();
        let second = staging(root).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.parent(), root.parent());
        let first_name = first.file_name().unwrap().to_string_lossy();
        let second_name = second.file_name().unwrap().to_string_lossy();
        assert!(first_name.starts_with("export-cut.partial-"));
        assert!(second_name.starts_with("export-cut.partial-"));
        assert_ne!(first_name, "export-cut.partial");
    }

    #[cfg(unix)]
    #[test]
    fn a_published_bundle_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        use crate::backup::application::PublishedRecord;

        let root = tempfile::tempdir().unwrap();
        let published = root.path().join("cut");
        let records = vec![PublishedRecord {
            store: StoreKind::ConversationAudit,
            identity: "creation/note.json".to_owned(),
            bytes: b"audit".to_vec(),
        }];
        write_bundle(&published, "manifest-line\n", &records).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&published), 0o700);
        assert_eq!(mode(&published.join("manifest.txt")), 0o600);
        assert_eq!(mode(&published.join("records")), 0o700);
        assert_eq!(mode(&published.join("records/conversation-audit")), 0o700);
        let file = published
            .join("records/conversation-audit")
            .join(encode("creation/note.json"));
        assert_eq!(mode(&file), 0o600);
        assert_eq!(fs::read(file).unwrap(), b"audit");
        assert_eq!(
            fs::read(published.join("manifest.txt")).unwrap(),
            b"manifest-line\n"
        );
    }
}
