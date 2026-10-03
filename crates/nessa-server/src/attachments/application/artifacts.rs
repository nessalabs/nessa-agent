#![deny(missing_docs)]
//! Local content facts for an exact held attachment lifetime; no access grants.
use super::PortFuture;
use crate::{
    attachments::domain::{ArtifactId, Attachment},
    conversation::domain::ConversationId,
};
use nessa_auth::domain::OrganizationId;

/// Current content state; revision is derived from this state by the source adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactState {
    /// Exact stored bytes of one confirmed registration, revision one.
    Live(Attachment),
    /// The registration was retired, revision two.
    Deleted,
}
impl ArtifactState {
    /// The positive revision published for this immutable content lifetime.
    pub fn revision(&self) -> u64 {
        match self {
            Self::Live(_) => 1,
            Self::Deleted => 2,
        }
    }
}

/// Attachment-owned manifest facts, independent of receiving-device scope.
pub trait AttachmentArtifacts: Send + Sync {
    /// Read an exact registration. None is missing, not deletion or permission.
    /// The caller supplies actual current operation admission before network use.
    fn manifest<'a>(
        &'a self,
        organization: &'a OrganizationId,
        conversation: &'a ConversationId,
        id: &'a ArtifactId,
    ) -> PortFuture<'a, Option<ArtifactState>, ArtifactReadError>;

    /// Read one exact range after checking the current registration again.
    /// Returned bytes are not a complete verified receiver cache.
    fn chunk<'a>(
        &'a self,
        organization: &'a OrganizationId,
        conversation: &'a ConversationId,
        range: &'a ArtifactRange,
    ) -> PortFuture<'a, ArtifactBytes, ArtifactReadError>;

    /// Fence new artifact reads and await actual physical work completion.
    /// Cancelling this observer does not cancel the retained drain.
    fn shutdown(&self) -> PortFuture<'_, (), ArtifactReadError>;
}

/// Local source refusal; no variant grants access or infers deletion from I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactReadError {
    /// The single physical source slot is held; no request was queued.
    Busy,
    /// No confirmed registration exists for this exact identity.
    Missing,
    /// The exact lifetime has a durable retirement marker.
    Deleted,
    /// The expected revision or stored content differs from current facts.
    Changed,
    /// Revision, offset or chunk limit is outside source bounds.
    InvalidRequest,
    /// Current source facts or bytes cannot be established, or reads are fenced.
    Unavailable,
    /// The shared physical worker owner retained an unexpected panic.
    WorkerPanicked,
}

/// An immutable expected content version and bounded range request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRange {
    id: ArtifactId,
    revision: u64,
    stored: Attachment,
    offset: u64,
    max_bytes: usize,
}
impl ArtifactRange {
    /// Capture caller expectations. The source validates bounds and agreement
    /// before opening bytes; construction itself does not authorize a read.
    pub fn new(
        id: ArtifactId,
        revision: u64,
        stored: Attachment,
        offset: u64,
        max_bytes: usize,
    ) -> Self {
        Self {
            id,
            revision,
            stored,
            offset,
            max_bytes,
        }
    }
    /// Exact immutable registration identity.
    pub fn id(&self) -> &ArtifactId {
        &self.id
    }
    /// Expected source revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Exact expected normalized stored content.
    pub fn stored(&self) -> &Attachment {
        &self.stored
    }
    /// First requested byte.
    pub fn offset(&self) -> u64 {
        self.offset
    }
    /// Maximum requested data bytes.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

/// One bounded source range; complete content verification belongs to the receiver.
#[derive(Debug, PartialEq, Eq)]
pub struct ArtifactBytes(Box<[u8]>);
impl ArtifactBytes {
    /// Inspect the exact range without mutable access to its storage.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(bytes.into_boxed_slice())
    }
}
