//! The published bundle: a manifest and one file per public record.
//! Redacted records have no file.

use std::fs;
use std::path::{Path, PathBuf};

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
    let staging = staging(root)?;
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|_| ())?;
    for record in records {
        let path = record_path(&staging, record.store, &record.identity)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| ())?;
        }
        fs::write(&path, &record.bytes).map_err(|_| ())?;
    }
    fs::write(staging.join("manifest.txt"), manifest).map_err(|_| ())?;
    if root.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(());
    }
    fs::rename(&staging, root).map_err(|_| ())?;
    Ok(())
}

fn staging(root: &Path) -> Result<PathBuf, ()> {
    let name = root.file_name().ok_or(())?;
    let mut staged = name.to_os_string();
    staged.push(".partial");
    Ok(root.with_file_name(staged))
}

fn record_path(root: &Path, store: StoreKind, identity: &str) -> Result<PathBuf, ()> {
    let name = encode(identity);
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(());
    }
    Ok(root.join("records").join(store.as_str()).join(name))
}

fn encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0xf) as usize] as char);
            }
        }
    }
    out
}
