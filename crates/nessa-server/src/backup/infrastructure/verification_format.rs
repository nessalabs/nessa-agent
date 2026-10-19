//! The current persisted backup format shared by live storage and offline verification.

use super::deletion_decode::decode_inventory;
use super::manifest_decode::decode_manifest;
use crate::backup::application::{
    DecodedManifest, DeletionReadError, ManifestReadError, VerificationFormat,
};
use crate::backup::domain::DeletionLog;

pub struct ExportFormat;

impl VerificationFormat for ExportFormat {
    fn manifest(
        &self,
        text: &str,
        max_entries: usize,
    ) -> Result<DecodedManifest, ManifestReadError> {
        decode_manifest(text, max_entries)
    }
    fn inventory(&self, text: &str) -> Result<DeletionLog, DeletionReadError> {
        decode_inventory(text)
    }
}
