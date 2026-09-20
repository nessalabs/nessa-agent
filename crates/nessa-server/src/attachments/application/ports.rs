use crate::{
    attachments::domain::{Attachment, Caller, Hold, HoldState, UploadTicket},
    conversation::domain::ConversationId,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use std::{future::Future, pin::Pin};

/// A boxed port future with a typed failure.
pub type PortFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// The store could not be read or written. Nothing is said about why: callers
/// decide on the fact, and the adapter logs the detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreUnavailable;

/// How many bytes a staged transfer holds and what they hash to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceivedBytes {
    pub size: u64,
    pub digest: Sha256Digest,
}

/// What keeping a hold replaced, so the change can be taken back exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldChange {
    /// The hold this conversation already had on the same stored digest.
    pub previous: Option<Hold>,
}
impl HoldChange {
    pub fn before(&self) -> HoldState {
        if self.previous.is_some() {
            HoldState::Held
        } else {
            HoldState::Absent
        }
    }
}

/// What became of a stored file's bytes once a hold on them was released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobOutcome {
    /// Another conversation still holds the same bytes.
    StillHeld,
    /// This was the last hold; the bytes were removed.
    Removed,
    /// This was the last hold, and the bytes were already gone.
    Missing,
    /// This was the last hold, and the bytes could not be removed.
    RemovalFailed,
}

/// One hold that was removed, and what happened to its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleasedHold {
    pub hold: Hold,
    pub blob: BlobOutcome,
}

/// Everything a release did. A failure never stops the rest from being tried.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseReport {
    pub released: Vec<ReleasedHold>,
    /// Holds that could not be read or removed, and reference checks that
    /// could not be made. Each is left in place rather than guessed at.
    pub failures: usize,
}

/// A transfer being written to private temporary storage. Dropping it removes
/// what was written.
pub trait StagedUpload: Send {
    /// Append one chunk.
    fn write(&mut self, chunk: Vec<u8>) -> PortFuture<'_, (), StoreUnavailable>;
    /// Make the bytes durable and report what was written.
    fn finish(&mut self) -> PortFuture<'_, ReceivedBytes, StoreUnavailable>;
    /// Read back every finished byte. Callers bound the size before writing.
    fn read(&mut self) -> PortFuture<'_, Vec<u8>, StoreUnavailable>;
    /// Publish these bytes under their own digest and record `hold` on them, as
    /// one change no release can come between. Refused when the finished bytes
    /// are not exactly `hold.stored()`.
    fn keep(self: Box<Self>, hold: Hold) -> PortFuture<'static, HoldChange, StoreUnavailable>;
}

/// Content-addressed bytes and the conversations holding them.
pub trait AttachmentStore: Send + Sync {
    /// Whether the conversation keeps exactly this stored file, bytes included.
    fn holds<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        stored: &'a Attachment,
    ) -> PortFuture<'a, bool, StoreUnavailable>;
    /// The hold this conversation earned by uploading exactly this file, when
    /// its stored bytes are still there.
    fn find_upload<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        uploaded: &'a Attachment,
    ) -> PortFuture<'a, Option<Hold>, StoreUnavailable>;
    /// Begin writing a transfer.
    fn stage(&self) -> PortFuture<'_, Box<dyn StagedUpload>, StoreUnavailable>;
    /// Take back a kept hold: restore what it replaced, or remove it and any
    /// bytes nothing else holds.
    fn revert(&self, hold: Hold, change: HoldChange) -> PortFuture<'_, (), StoreUnavailable>;
    /// Remove every hold of one conversation, and bytes no hold remains on.
    fn release<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
    ) -> PortFuture<'a, ReleaseReport, StoreUnavailable>;
    /// At most `limit` bytes of the stored file with this digest, by content
    /// alone. `None` when no such bytes are stored.
    fn read(
        &self,
        digest: Sha256Digest,
        limit: u64,
    ) -> PortFuture<'_, Option<Vec<u8>>, StoreUnavailable>;
}

/// Why an image could not be brought into the form a conversation keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalizeError {
    /// The bytes are not an image this gateway can decode.
    Unsupported,
    /// The image cannot be brought under the selected model's limits.
    TooLarge,
    /// Normalization itself failed; the same upload may succeed later.
    Failed,
}

/// An image as the gateway will keep it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedImage {
    pub bytes: Vec<u8>,
    pub media_type: String,
}

/// Future returned by [`ImageNormalizer::normalize`].
pub type NormalizeFuture<'a> = PortFuture<'a, NormalizedImage, NormalizeError>;

/// Converts an uploaded image into what a conversation keeps: an encoding and
/// dimensions the selected model accepts. The implementation owns any blocking
/// work; it is called with the whole verified upload, once.
pub trait ImageNormalizer: Send + Sync {
    fn normalize<'a>(
        &'a self,
        original: Vec<u8>,
        declared_media_type: &'a str,
    ) -> NormalizeFuture<'a>;
}

/// The conversation's owner could not be looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnershipUnavailable;

/// Whether a conversation exists and belongs to this caller. Asked without
/// opening an agent: beginning an upload must not start a provider.
pub trait ConversationOwnership: Send + Sync {
    fn owns<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        principal_id: &'a PrincipalId,
        conversation_id: &'a ConversationId,
    ) -> PortFuture<'a, bool, OwnershipUnavailable>;
}

/// The host could not supply randomness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretsUnavailable;

/// Where ticket secrets come from: 32 unpredictable bytes each.
pub trait TicketSecrets: Send + Sync {
    fn fresh(&self) -> Result<[u8; 32], SecretsUnavailable>;
}

/// The audit sink did not acknowledge a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditUnavailable;

/// Why a redeemed ticket did not become a hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadRejection {
    /// More or fewer bytes than the ticket described.
    SizeMismatch,
    /// The right number of bytes, hashing to something else.
    DigestMismatch,
    /// The transfer ended before it was complete.
    BodyInterrupted,
    /// The transfer did not finish within the upload deadline.
    DeadlineElapsed,
    /// The bytes could not be written, published, or held.
    StorageUnavailable,
    /// Declared an image, but not one this gateway can decode.
    UnsupportedImage,
    /// An image that cannot be brought under the selected model's limits.
    ImageTooLarge,
    /// Normalization failed, or produced something that is not a storable image.
    NormalizationFailed,
}

/// Why holds were released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseCause {
    /// The verified caller closed the conversation.
    ConversationClosed,
}

/// The verified request behind a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseEvidence {
    pub cause: ReleaseCause,
    pub caller: Caller,
    pub requested_at_ms: u64,
}

/// One consequential transition. Each variant carries the whole value it
/// happened to, so target, owner, both digests, initiator, correlation, and
/// the time of the causing request all travel together. The sink assigns the
/// record's identity and the time it observed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentAuditRecord {
    /// A verified upload became a hold. Initiated by the ticket's caller.
    HoldCreated { hold: Hold, before: HoldState },
    /// A redeemed ticket was used up without producing a hold.
    UploadRejected {
        ticket: UploadTicket,
        reason: UploadRejection,
    },
    /// A ticket's time passed unused. Nobody did this; it is automatic.
    TicketExpired { ticket: UploadTicket },
    /// A ticket was withdrawn unused because its conversation let go of its files.
    TicketVoided {
        ticket: UploadTicket,
        release: ReleaseEvidence,
    },
    /// A conversation let go of a stored file.
    HoldReleased {
        hold: Hold,
        release: ReleaseEvidence,
    },
    /// The last hold on some bytes was released, so the bytes were removed.
    /// `hold` is that last hold.
    BlobRemoved {
        hold: Hold,
        release: ReleaseEvidence,
    },
}

/// Durable evidence of attachment transitions, committed before success is reported.
pub trait AttachmentAudit: Send + Sync {
    fn record(&self, record: AttachmentAuditRecord) -> PortFuture<'_, (), AuditUnavailable>;
}

/// The transfer ended early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyInterrupted;

/// The bytes of one upload, a chunk at a time, however they arrive.
pub trait UploadBody: Send {
    /// The next chunk, or `None` once the transfer is complete.
    fn next(&mut self) -> PortFuture<'_, Option<Vec<u8>>, BodyInterrupted>;
}
