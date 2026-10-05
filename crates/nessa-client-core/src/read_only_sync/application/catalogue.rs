//! Passive retained catalogue presentation; no source authority or live controls.
use nessa_protocol::conversation::catalogue_metadata::CatalogueMetadata;
use nessa_sync::replication::catalogue::{CatalogueProgress, EntryKey, ManifestEntry};

#[derive(Debug)]
pub(crate) struct CachedCatalogueEntry {
    manifest: ManifestEntry,
    metadata: Option<CatalogueMetadata>,
}

impl CachedCatalogueEntry {
    /// The physical cache supplies a descriptor and metadata admitted together
    /// by its stored-entry reader; this constructor performs no I/O.
    pub(crate) fn new(manifest: ManifestEntry, metadata: Option<CatalogueMetadata>) -> Self {
        Self { manifest, metadata }
    }
    pub(crate) fn manifest(&self) -> &ManifestEntry {
        &self.manifest
    }
    pub(crate) fn metadata(&self) -> Option<&CatalogueMetadata> {
        self.metadata.as_ref()
    }
}

#[derive(Debug)]
pub(crate) struct CachedCataloguePage {
    progress: CatalogueProgress,
    entries: Box<[CachedCatalogueEntry]>,
    next: Option<EntryKey>,
}

impl CachedCataloguePage {
    pub(crate) fn new(
        progress: CatalogueProgress,
        entries: Box<[CachedCatalogueEntry]>,
        next: Option<EntryKey>,
    ) -> Self {
        Self {
            progress,
            entries,
            next,
        }
    }
    pub(crate) fn progress(&self) -> &CatalogueProgress {
        &self.progress
    }
    pub(crate) fn entries(&self) -> &[CachedCatalogueEntry] {
        &self.entries
    }
    pub(crate) fn next(&self) -> Option<&EntryKey> {
        self.next.as_ref()
    }
}
