//! Issuing tickets, receiving uploads, and releasing holds, through ports the
//! infrastructure fills in.
//!
//! `AttachmentService` is the one owner of every transition here: a ticket is
//! issued, replaced, redeemed, expired, or withdrawn; a hold is written pending,
//! recorded, and only then confirmed, or taken back by the upload that wrote it,
//! or released; bytes are removed when their last hold goes. Each is handed to the
//! `AttachmentAudit` port before success is reported, and an audit failure is
//! reported without stopping cleanup. A bulk phase (a release, or one sweep of
//! expired tickets) gives every record its own deadline; the caller waits at
//! most the phase budget and does not cancel the attempts by returning or by
//! being dropped. Dropping the service aborts attempts not yet in the sink. The
//! service holds each bulk attempt's admission permit until that attempt's
//! deadline and does not hand the permit to the sink, so a write that has
//! already started cannot take the next record's attempt. The order is
//! `docs/design/artifact-sync.md`.
//!
//! ```text
//! begin   -> ConversationOwnership, AttachmentStore::find_upload, TicketSecrets -> TicketBook
//! receive -> TicketBook -> UploadBody -> StagedUpload -> ImageNormalizer -> StagedUpload::keep
//!            -> AttachmentAudit (created) -> ConversationOwnership
//!            -> AttachmentStore::confirm | discard
//! release -> TicketBook (withdraw) -> AttachmentStore::release
//! every transition -> AttachmentAudit
//! ```
//!
//! An arrow is a call. Nothing on the right knows about the service.
mod artifacts;
mod error;
mod ports;
mod service;
mod ticket_secret;

pub use artifacts::{
    ArtifactBytes, ArtifactRange, ArtifactReadError, ArtifactState, AttachmentArtifacts,
};
pub use error::{AuditDelivery, BeginError, ReleaseError, UploadError};
pub use ports::{
    AttachmentAudit, AttachmentAuditRecord, AttachmentStore, AuditUnavailable, Confirmation,
    ConversationOwnership, Discard, HoldClaim, ImageNormalizer, Kept, NormalizeError,
    NormalizeFuture, NormalizedImage, Ownership, OwnershipUnavailable, PortFuture, PublishKept,
    PublishedFile, ReceivedBytes, ReleaseCause, ReleaseEvidence, ReleaseReport, RemovedBlob,
    RetiredHold, RetirementEvidence, RevertCause, SecretsUnavailable, StagedUpload,
    StoreUnavailable, TicketSecrets, UploadBody, UploadInterrupted, UploadRejection,
};
pub use service::{
    AttachmentCaller, AttachmentDependencies, AttachmentLimits, AttachmentService, BeginOutcome,
    BeginUpload, ReleaseRequest,
};
pub use ticket_secret::TicketSecret;

#[cfg(test)]
#[path = "../../../tests/attachments/application.rs"]
mod tests;
