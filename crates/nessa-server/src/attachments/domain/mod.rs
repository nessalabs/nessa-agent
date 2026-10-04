//! What an uploaded file is, who may upload it, and who holds it. No clock
//! reads, no files, no transport: time arrives as a number and secrets arrive
//! already hashed.
//!
//! `value_objects/` describe a file (`Attachment`, `MediaType`), the verified
//! caller behind an action (`Caller`), and one permission to upload
//! (`UploadTicket`, its `TicketLifetime`, and the `TicketFingerprint` it is
//! found by). `entities/` owns the `Hold`: one conversation keeping one stored
//! file, remembering what was uploaded to produce it, and `RetiredFrom`, the
//! existing Pending/Held predecessor shared by retirement ports and storage.
//! `aggregates/` owns the
//! `TicketBook`, the one place where "single use", "expires", and "at most this
//! many outstanding" are decided together.
pub mod aggregates;
pub mod entities;
mod error;
pub mod value_objects;

pub use aggregates::{BookFull, Issued, Redemption, TicketBook, TicketLimits};
pub use entities::{Hold, HoldState, RetiredFrom};
pub use error::AttachmentError;
pub use value_objects::{
    ArtifactId, Attachment, Caller, InvalidArtifactId, MediaType, TicketFingerprint,
    TicketLifetime, UploadMismatch, UploadTicket, TICKET_LIFETIME_MS,
};

#[cfg(test)]
#[path = "../../../tests/attachments/domain.rs"]
mod tests;
