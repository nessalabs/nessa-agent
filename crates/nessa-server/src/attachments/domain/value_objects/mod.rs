//! Immutable, validated descriptions. A change produces a replacement value.
mod attachment;
mod caller;
mod media_type;
mod ticket;

pub use attachment::Attachment;
pub use caller::Caller;
pub use media_type::MediaType;
pub use ticket::{
    TicketFingerprint, TicketLifetime, UploadMismatch, UploadTicket, TICKET_LIFETIME_MS,
};
