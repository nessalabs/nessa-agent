//! Typed results at the persisted-format boundary; composition supplies the decoder.

use crate::backup::domain::{DeletionLog, Fingerprint, ManifestListing};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestReadError {
    UnsupportedVersion,
    Malformed,
    EntryLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeletionReadError {
    UnsupportedVersion,
    Malformed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedManifest {
    pub listing: ManifestListing,
    /// Byte offset immediately before the final mac line, in the supplied text.
    pub canonical_len: usize,
    pub mac: Fingerprint,
}

pub trait VerificationFormat {
    fn manifest(
        &self,
        text: &str,
        max_entries: usize,
    ) -> Result<DecodedManifest, ManifestReadError>;
    fn inventory(&self, text: &str) -> Result<DeletionLog, DeletionReadError>;
}
