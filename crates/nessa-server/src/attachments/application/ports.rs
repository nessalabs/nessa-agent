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

/// Proof of having written one particular pending hold. The store issues it,
/// and only its bearer may make that hold usable or take it back. An upload
/// can therefore undo exactly the transition it made and no other: not a later
/// upload of the same file, and not a hold a release already removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldClaim(Box<str>);
impl HoldClaim {
    pub fn new(generation: &str) -> Self {
        Self(generation.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What keeping staged bytes did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// The bytes are published and a hold on them is written, but it is not
    /// usable: nothing that asks what a conversation holds can see it until
    /// its claim is confirmed.
    Pending(HoldClaim),
    /// The conversation already keeps exactly this stored file. Nothing
    /// changed; the hold is returned as it stands.
    Existing(Hold),
}

/// What confirming a claim found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    /// The pending hold is now usable, as this claim's hold.
    Confirmed,
    /// Another upload of the same file made the hold usable first.
    AlreadyKept,
    /// The hold is gone: its conversation let go of its files meanwhile.
    Gone,
}

/// What taking a claim back found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discard {
    /// The hold this claim wrote was removed, and bytes nothing else holds.
    Discarded,
    /// Nothing of this claim remains: another upload took the hold over, or a
    /// release removed it. Nothing was touched.
    NotMine,
}

/// One hold a release removed, and whether it had become usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleasedHold {
    pub hold: Hold,
    /// [`HoldState::Held`], or [`HoldState::Pending`] for a hold whose upload
    /// had not finished recording it.
    pub was: HoldState,
}

/// Everything a release did. A failure never stops the rest from being tried.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseReport {
    pub released: Vec<ReleasedHold>,
    /// For each stored digest whose bytes were removed, the last hold on it.
    pub removed: Vec<Hold>,
    /// Holds that could not be read or removed, and bytes that could not be
    /// removed or proved unheld. Each is left in place rather than guessed at.
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
    /// Publish these bytes under their own digest and write a pending hold on
    /// them, as one change no release can come between. Refused when the
    /// finished bytes are not exactly `hold.stored()`. A pending hold another
    /// upload left is replaced, so its claim no longer matches; a usable hold
    /// is never replaced.
    fn keep(self: Box<Self>, hold: Hold) -> PortFuture<'static, Kept, StoreUnavailable>;
}

/// Content-addressed bytes and the conversations holding them.
pub trait AttachmentStore: Send + Sync {
    /// Whether the conversation keeps exactly this stored file, usably, bytes
    /// included. A pending hold does not count.
    fn holds<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        stored: &'a Attachment,
    ) -> PortFuture<'a, bool, StoreUnavailable>;
    /// The usable hold this conversation earned by uploading exactly this
    /// file, when its stored bytes are still there.
    fn find_upload<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        uploaded: &'a Attachment,
    ) -> PortFuture<'a, Option<Hold>, StoreUnavailable>;
    /// Begin writing a transfer.
    fn stage(&self) -> PortFuture<'_, Box<dyn StagedUpload>, StoreUnavailable>;
    /// Make the pending hold usable. A pending hold under another claim is
    /// taken over: this caller has committed evidence for the same file.
    fn confirm<'a>(
        &'a self,
        hold: &'a Hold,
        claim: &'a HoldClaim,
    ) -> PortFuture<'a, Confirmation, StoreUnavailable>;
    /// Take back the hold this claim wrote, if it is still this claim's.
    fn discard<'a>(
        &'a self,
        hold: &'a Hold,
        claim: &'a HoldClaim,
    ) -> PortFuture<'a, Discard, StoreUnavailable>;
    /// Remove every hold of one conversation, pending or usable, and bytes no
    /// hold remains on.
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
    /// No image is prepared for the selected model: it records no image
    /// limits, so it is offered none. Nothing is wrong with the upload.
    NotOffered,
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
/// work; it is called with the whole verified upload, once. What it returns is
/// checked by the caller: it must be an image a message can name.
pub trait ImageNormalizer: Send + Sync {
    /// Whether any image is prepared for the selected model. `false` is a model
    /// with no recorded image limits: it is offered none, so an upload that
    /// says it is an image is refused before a ticket is issued rather than
    /// normalized into something no message could ever name.
    fn offers_images(&self) -> bool;
    /// Fit `original` to the selected model. What the bytes are is read from
    /// the bytes; the client's declared media type is not an input.
    fn normalize(&self, original: Vec<u8>) -> NormalizeFuture<'_>;
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

/// Why a redeemed ticket did not become a hold. Each is named as the upload
/// route names it, so the wire, the audit trail, and the guide use one word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadRejection {
    /// More or fewer bytes than the ticket described.
    SizeMismatch,
    /// The right number of bytes, hashing to something else.
    DigestMismatch,
    /// The transfer ended before it was complete.
    UploadInterrupted,
    /// The transfer did not finish within the upload deadline.
    UploadTimeout,
    /// The bytes could not be written, published, or held.
    StorageUnavailable,
    /// The work of this upload stopped without an answer, so what the bytes
    /// were is unknown. Nothing is claimed about them either way.
    Unresolved,
    /// The selected model is offered no images, so none is prepared for it.
    ImageInputUnsupported,
    /// Declared an image, but not one this gateway can decode.
    UnsupportedImage,
    /// An image that cannot be brought under the selected model's limits.
    ImageTooLarge,
    /// Normalization failed, or produced something no message could name.
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

/// Why a pending hold was taken back by the upload that wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevertCause {
    /// Its creation was not acknowledged by the audit sink. The sink may have
    /// committed that record anyway, which is why this one follows it.
    AuditUnconfirmed,
    /// Its creation was recorded, but the hold could not be made usable.
    ConfirmationFailed,
    /// Its creation was recorded, but the hold was gone before it could be
    /// made usable: released with its conversation, or taken back by another
    /// upload of the same file.
    RemovedBeforeUsable,
    /// The work of the upload that wrote it stopped without an answer, so
    /// nobody was left to finish or undo it.
    UploadUnresolved,
}

/// One consequential transition. Each variant carries the whole value it
/// happened to, so target, owner, both digests, initiator, correlation, and
/// the time of the causing request all travel together. The sink assigns the
/// record's identity and the time it observed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentAuditRecord {
    /// The authenticated socket gave a verified caller permission to upload
    /// one file. Recorded before the ticket is handed out.
    TicketIssued { ticket: UploadTicket },
    /// The same request was made again, so its earlier ticket stopped working.
    TicketReplaced { ticket: UploadTicket },
    /// A ticket's time passed unused. Nobody did this; it is automatic.
    TicketExpired { ticket: UploadTicket },
    /// A ticket was withdrawn unused because its conversation let go of its files.
    TicketWithdrawn {
        ticket: UploadTicket,
        release: ReleaseEvidence,
    },
    /// A redeemed ticket was used up without producing a hold.
    UploadRejected {
        ticket: UploadTicket,
        reason: UploadRejection,
    },
    /// A verified upload became a hold. Initiated by the ticket's caller.
    /// Recorded while the hold is pending; the hold becomes usable after.
    HoldCreated { hold: Hold },
    /// A verified upload arrived at a file the conversation already keeps.
    /// The ticket is used; `hold` is the unchanged hold.
    AlreadyHeld { ticket: UploadTicket, hold: Hold },
    /// A pending hold was taken back by its own upload. Automatic.
    HoldReverted { hold: Hold, cause: RevertCause },
    /// A conversation let go of a stored file.
    HoldReleased {
        hold: Hold,
        was: HoldState,
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
pub struct UploadInterrupted;

/// The bytes of one upload, a chunk at a time, however they arrive.
pub trait UploadBody: Send {
    /// The next chunk, or `None` once the transfer is complete.
    fn next(&mut self) -> PortFuture<'_, Option<Vec<u8>>, UploadInterrupted>;
}
