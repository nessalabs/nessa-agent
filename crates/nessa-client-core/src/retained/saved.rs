//! Typed snapshots delegate selection and projection to the existing private
//! cache. They hold no transport, enrollment or second checkpoint authority.
use super::{cache_failure, ReadFailure, RetainedCache};
use crate::read_only_sync::application::{
    offline::{SavedReads, SavedTranscript},
    CachedProgress,
};
use nessa_protocol::conversation::{
    catalogue_metadata::CatalogueMetadata, domain::ConversationId, view::ConversationView,
};
use nessa_sync::replication::{
    catalogue::{CatalogueProgress, EntryKey, ManifestEntry},
    domain::{Id, Scope},
};
use uuid::Uuid;

/// An owned catalogue row, including the retained deletion descriptor.
/// Metadata validity remains the cache's existing codec decision.
#[derive(Clone, Debug)]
pub struct RetainedCatalogueEntry {
    manifest: ManifestEntry,
    metadata: Option<CatalogueMetadata>,
}
impl RetainedCatalogueEntry {
    /// The physical identity, revision and deletion marker the cache retained.
    pub fn manifest(&self) -> &ManifestEntry {
        &self.manifest
    }
    /// Validated metadata, absent for a retained deletion marker.
    pub fn metadata(&self) -> Option<&CatalogueMetadata> {
        self.metadata.as_ref()
    }
}

/// One bounded, owned page from one database snapshot. A later call can observe
/// another scope, generation or completed revision; callers compare progress
/// before describing multiple pages as one traversal. An active pass is partial.
#[derive(Clone, Debug)]
pub struct RetainedCataloguePage {
    progress: CatalogueProgress,
    entries: Box<[RetainedCatalogueEntry]>,
    next: Option<EntryKey>,
}
impl RetainedCataloguePage {
    /// Actual saved scope, completed revision, active pass and generation.
    pub fn progress(&self) -> &CatalogueProgress {
        &self.progress
    }
    /// The selected rows, including tombstones, in physical key order.
    pub fn entries(&self) -> &[RetainedCatalogueEntry] {
        &self.entries
    }
    /// The last selected key if more saved rows were observed in this snapshot.
    pub fn next(&self) -> Option<&EntryKey> {
        self.next.as_ref()
    }
}

/// Distinct physical download, semantic apply and visible-fact progress.
/// These values are saved cache evidence, not current gateway freshness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedProgress(CachedProgress);
impl RetainedProgress {
    /// The exact receiver, origin, stream, incarnation, schema and access epoch.
    pub fn scope(&self) -> &Scope {
        &self.0.scope
    }
    /// Last contiguous physical position retained on disk.
    pub fn downloaded(&self) -> u64 {
        self.0.downloaded
    }
    /// Last physical position incorporated by the shared semantic fold.
    pub fn applied(&self) -> u64 {
        self.0.applied
    }
    /// Last physical position that contributed visible semantic facts.
    pub fn facts(&self) -> u64 {
        self.0.facts
    }
    /// The durable cache generation from which this snapshot was read.
    pub fn generation(&self) -> u64 {
        self.0.generation
    }
}

/// A saved transcript's meaning. An empty retained view differs from no data.
/// Returned views are owned passive snapshots and cannot update after later
/// withdrawal. Their view revision is presentation identity, not a checkpoint.
#[derive(Debug)]
pub enum RetainedTranscript {
    /// No readable checkpoint exists for this selected target.
    NotLoaded,
    /// A retained deletion fence exists for this target.
    Deleted,
    /// Existing shared projection of the saved checkpoint and physical suffix.
    Retained(RetainedTranscriptSnapshot),
}

/// One immutable progress/view pair produced by the existing saved-read owner.
/// Borrowed accessors prevent callers from altering the view while retaining
/// this snapshot's saved progress.
///
/// ```compile_fail
/// use nessa_client_core::retained::RetainedTranscript;
/// fn alter(evidence: RetainedTranscript) {
///     if let RetainedTranscript::Retained(mut snapshot) = evidence {
///         snapshot.view().messages.clear();
///     }
/// }
/// ```
#[derive(Debug)]
pub struct RetainedTranscriptSnapshot {
    progress: RetainedProgress,
    view: Box<ConversationView>,
}
impl RetainedTranscriptSnapshot {
    /// Actual saved progress, independently of the presentation revision.
    pub fn progress(&self) -> &RetainedProgress {
        &self.progress
    }
    /// Passive product view with no authority to execute live controls.
    pub fn view(&self) -> &ConversationView {
        &self.view
    }
}

impl RetainedCache {
    /// Discover the actual saved catalogue scope in one cache snapshot. This
    /// is evidence for target selection, not a claim that a later page shares
    /// the same snapshot. Pages return their own scope and generation.
    pub fn saved_catalogue_scope(&mut self, receiver: &str) -> Result<Option<Scope>, ReadFailure> {
        let receiver = Id::new(receiver).map_err(|_| ReadFailure::Protocol)?;
        self.catalogue_scope(&receiver)
    }

    /// Select a bounded saved page for this explicit receiver/origin/stream.
    /// Performs no network request. `max_entries` is validated by the existing
    /// cache owner against the published catalogue limit. `None` means no saved
    /// catalogue, while an empty page can represent a retained empty catalogue.
    pub fn catalogue_page(
        &mut self,
        receiver: &str,
        origin: &str,
        stream: &str,
        after: Option<&EntryKey>,
        max_entries: usize,
    ) -> Result<Option<RetainedCataloguePage>, ReadFailure> {
        let receiver = Id::new(receiver).map_err(|_| ReadFailure::Protocol)?;
        let origin = Id::new(origin).map_err(|_| ReadFailure::Protocol)?;
        let stream = Id::new(stream).map_err(|_| ReadFailure::Protocol)?;
        let page = self
            .cache
            .catalogue_page(&receiver, &origin, &stream, after, max_entries)
            .map_err(|error| cache_failure(&error))?;
        Ok(page.map(|page| RetainedCataloguePage {
            progress: page.progress().clone(),
            entries: page
                .entries()
                .iter()
                .map(|entry| RetainedCatalogueEntry {
                    manifest: entry.manifest().clone(),
                    metadata: entry.metadata().cloned(),
                })
                .collect(),
            next: page.next().cloned(),
        }))
    }

    /// Project a selected saved target without connecting or asserting current
    /// authority. `origin` comes from actual retained scope evidence, such as a
    /// catalogue page. The existing saved-read owner restores scope/checkpoint,
    /// refuses corruption, applies the contiguous suffix and checks tombstones.
    pub fn transcript_view(
        &mut self,
        receiver: &str,
        origin: &str,
        conversation: &ConversationId,
        revision: Uuid,
    ) -> Result<RetainedTranscript, ReadFailure> {
        let receiver = Id::new(receiver).map_err(|_| ReadFailure::Protocol)?;
        let origin = Id::new(origin).map_err(|_| ReadFailure::Protocol)?;
        match self
            .cache
            .transcript_view(&receiver, &origin, conversation, revision)
            .map_err(|error| cache_failure(&error))?
        {
            SavedTranscript::NotLoaded => Ok(RetainedTranscript::NotLoaded),
            SavedTranscript::Deleted => Ok(RetainedTranscript::Deleted),
            SavedTranscript::Retained { progress, view } => {
                Ok(RetainedTranscript::Retained(RetainedTranscriptSnapshot {
                    progress: RetainedProgress(progress),
                    view,
                }))
            }
        }
    }
}
