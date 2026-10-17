//! An authentic bundle and explicit private key, containing synthetic bytes only.

use std::io::Write;
use std::path::{Path, PathBuf};

use nessa_local_storage::{open, OpenMode};
use tempfile::TempDir;

use super::server::backup::application::{
    Authenticator, MacKey, ManifestPublisher, PublishedRecord,
};
use super::server::backup::domain::{
    AuthenticatedManifest, CapturedRecord, Manifest, Sensitivity, SourceCut, StoreKind, Watermark,
    WRITER_INVENTORY,
};
use super::server::backup::infrastructure::DirectoryPublisher;

pub const BODY: &[u8] = b"synthetic saved conversation bytes\n";

pub struct Fixture {
    pub parent: TempDir,
    pub bundle: PathBuf,
    pub key_file: PathBuf,
    pub key: MacKey,
}

impl Fixture {
    pub async fn published() -> Self {
        Self::publish(true).await
    }

    pub async fn empty() -> Self {
        Self::publish(false).await
    }

    async fn publish(include_bodies: bool) -> Self {
        let parent = tempfile::tempdir().unwrap();
        let bundle = parent.path().join("bundle");
        let key_file = parent.path().join("explicit-mac-key");
        let key = MacKey([37; 32]);
        private_file(&key_file, &key.0);
        let kind = StoreKind::ConversationMetadata;
        let cuts = WRITER_INVENTORY
            .iter()
            .map(|store| {
                if *store == kind && include_bodies {
                    SourceCut::new(
                        kind,
                        Watermark::new("synthetic-cut").unwrap(),
                        vec![
                            CapturedRecord::new(
                                kind,
                                "public-row",
                                Sensitivity::Public,
                                BODY.to_vec(),
                            )
                            .unwrap(),
                            CapturedRecord::new(
                                kind,
                                "redacted-row",
                                Sensitivity::Secret,
                                b"SYNTHETIC-SECRET".to_vec(),
                            )
                            .unwrap(),
                        ],
                        vec![],
                    )
                    .unwrap()
                } else {
                    SourceCut::absent(*store).unwrap()
                }
            })
            .collect();
        let manifest = Manifest::new(123, cuts).unwrap();
        let mac = key.mac(&manifest.canonical()).unwrap();
        let encoded = AuthenticatedManifest::seal(manifest, mac).encoded();
        let records = if include_bodies {
            vec![PublishedRecord {
                store: kind,
                identity: "public-row".to_owned(),
                bytes: BODY.to_vec(),
            }]
        } else {
            Vec::new()
        };
        DirectoryPublisher::new(&bundle)
            .put(&encoded, &records)
            .await
            .unwrap();
        Self {
            parent,
            bundle,
            key_file,
            key,
        }
    }

    pub fn body(&self) -> PathBuf {
        self.store().join("public-row")
    }

    pub fn store(&self) -> PathBuf {
        self.bundle
            .join("records")
            .join(StoreKind::ConversationMetadata.as_str())
    }
}

pub fn private_file(path: &Path, bytes: &[u8]) {
    open(path, OpenMode::CreateNew)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}
