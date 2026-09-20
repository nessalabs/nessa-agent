//! Issuing tickets, receiving uploads, and releasing holds, through ports the
//! infrastructure fills in.
//!
//! `AttachmentService` is the one owner of every transition here: a ticket is
//! issued, redeemed, expired, or withdrawn; a hold is created, taken back, or
//! released; bytes are removed when their last hold goes. Each is handed to the
//! `AttachmentAudit` port before success is reported, and an audit failure is
//! reported without stopping cleanup.
//!
//! ```text
//! begin   -> ConversationOwnership, AttachmentStore::find_upload, TicketSecrets -> TicketBook
//! receive -> TicketBook -> UploadBody -> StagedUpload -> ImageNormalizer -> StagedUpload::keep
//! release -> TicketBook (withdraw) -> AttachmentStore::release
//! every transition -> AttachmentAudit
//! ```
//!
//! An arrow is a call. Nothing on the right knows about the service.
mod error;
mod ports;
mod service;
mod ticket_secret;

pub use error::{AuditDelivery, BeginError, ReleaseError, UploadError};
pub use ports::{
    AttachmentAudit, AttachmentAuditRecord, AttachmentStore, AuditUnavailable, BlobOutcome,
    BodyInterrupted, ConversationOwnership, HoldChange, ImageNormalizer, NormalizeError,
    NormalizeFuture, NormalizedImage, OwnershipUnavailable, PortFuture, ReceivedBytes,
    ReleaseCause, ReleaseEvidence, ReleaseReport, ReleasedHold, SecretsUnavailable, StagedUpload,
    StoreUnavailable, TicketSecrets, UploadBody, UploadRejection,
};
pub use service::{
    AttachmentCaller, AttachmentDependencies, AttachmentLimits, AttachmentService, BeginOutcome,
    BeginUpload, ReleaseRequest,
};
pub use ticket_secret::TicketSecret;

#[cfg(test)]
#[path = "../../../tests/attachments/application.rs"]
mod tests;
