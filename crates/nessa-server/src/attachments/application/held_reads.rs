//! Reading a file a conversation holds back to a surface, one bounded range
//! at a time (`artifact.read`, slice D2 of #701).
//!
//! ```text
//! product socket --artifact.read--> HeldReads --owns--> ConversationOwnership
//!   (conversation admitted first)        └--read_held--> AttachmentArtifacts --> bytes
//!                                             the hold for exactly that file, now
//! ```
//!
//! The socket admits the caller to the conversation before it asks (a paired
//! device reads only what it was shared). Here the conversation must still
//! be the caller's, asked as an upload asks it, and what it may read is then
//! decided by the one authority on which bytes a conversation keeps: its
//! hold for exactly that file (digest, media type and size together). A file
//! the conversation never held, or has let go of, reads nothing, whatever
//! else names its digest. Every range asks again, in the same step that
//! opens the bytes, so a hold released between two ranges stops the read
//! there (`not held`), and no range of other bytes is ever served under its
//! name. A file released and kept again between two ranges reads on: it is
//! held, and its bytes are the ones its digest names.
//!
//! Nothing here verifies the whole file: a range is a range. The receiver
//! checks the size and the digest of what it assembled, which is the only
//! check that holds for what arrived.
use super::{
    ArtifactBytes, ArtifactReadError, AttachmentArtifacts, ConversationOwnership, Ownership,
};
use crate::attachments::domain::Attachment;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use std::{sync::Arc, time::Duration};

/// Why a range was not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldReadError {
    /// An offset at or past the end, or a range of no bytes or more than
    /// [`HeldReads::MAX_RANGE_BYTES`].
    InvalidRequest,
    /// Not the caller's conversation, or not there at all.
    ConversationNotFound,
    /// The caller's conversation, deleted: it holds nothing.
    ConversationDeleted,
    /// The conversation does not keep exactly this file now: never held,
    /// still pending, released, or kept as other bytes.
    NotHeld,
    /// The source's one slot stayed taken by reads ahead of this one for
    /// [`HeldReads::BUSY_WAIT`]; the same range may be asked again.
    Busy,
    /// The store could not say, or could not read the bytes.
    Unavailable,
    /// Whose conversation it is could not be established now.
    OwnershipUnavailable,
}

/// Reads held files by range. Cheap to clone; every clone reads the same
/// source.
#[derive(Clone)]
pub struct HeldReads {
    source: Arc<dyn AttachmentArtifacts>,
    ownership: Arc<dyn ConversationOwnership>,
}

impl HeldReads {
    /// The most bytes one range carries. Its base64 in an ordinary product
    /// response, with the answer's other fields, stays inside the socket's
    /// 64 KiB frame, which is the smaller of the two frames a surface reads
    /// it over.
    pub const MAX_RANGE_BYTES: usize = 32 * 1024;
    /// How long a range waits, queued behind the ranges ahead of it, for the
    /// source's one read slot before it answers busy. The slot is held for
    /// one record read and one range's disk read.
    pub const BUSY_WAIT: Duration = Duration::from_secs(2);
    /// How many ranges the gateway reads, or waits to read, at once, across
    /// every surface; one more is answered `temporarily_unavailable` at once.
    /// A surface reads its ranges one after another.
    pub const CONCURRENT_READS: usize = 4;

    pub fn new(
        source: Arc<dyn AttachmentArtifacts>,
        ownership: Arc<dyn ConversationOwnership>,
    ) -> Self {
        Self { source, ownership }
    }

    /// At most `max_bytes` of `stored`, from `offset`, when `conversation`
    /// is `principal`'s and keeps exactly that file.
    pub async fn read(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        conversation: &ConversationId,
        stored: &Attachment,
        offset: u64,
        max_bytes: usize,
    ) -> Result<ArtifactBytes, HeldReadError> {
        if max_bytes == 0 || max_bytes > Self::MAX_RANGE_BYTES || offset >= stored.size() {
            return Err(HeldReadError::InvalidRequest);
        }
        match self
            .ownership
            .owns(organization, principal, conversation)
            .await
        {
            Ok(Ownership::Owned) => {}
            Ok(Ownership::NotFound) => return Err(HeldReadError::ConversationNotFound),
            Ok(Ownership::Deleted) => return Err(HeldReadError::ConversationDeleted),
            Err(_) => return Err(HeldReadError::OwnershipUnavailable),
        }
        self.source
            .read_held(
                organization,
                conversation,
                stored,
                offset,
                max_bytes,
                Self::BUSY_WAIT,
            )
            .await
            .map_err(refusal)
    }

    /// Fence new reads and wait for the one under way to finish. What could
    /// not be confirmed is logged: the store is let go of either way.
    pub async fn shutdown(&self) {
        if let Err(error) = self.source.shutdown().await {
            tracing::error!(?error, "held file reads did not drain at shutdown");
        }
    }
}

fn refusal(error: ArtifactReadError) -> HeldReadError {
    match error {
        ArtifactReadError::Busy => HeldReadError::Busy,
        // Pending, retired, absent, or other bytes under its name: all one
        // fact to the reader, that the conversation does not keep this file.
        ArtifactReadError::Missing | ArtifactReadError::Deleted | ArtifactReadError::Changed => {
            HeldReadError::NotHeld
        }
        // Checked above, so the source refusing the range is its own fault.
        // A worker panic fences the source's reads until the gateway restarts.
        ArtifactReadError::InvalidRequest
        | ArtifactReadError::Unavailable
        | ArtifactReadError::WorkerPanicked => HeldReadError::Unavailable,
    }
}
