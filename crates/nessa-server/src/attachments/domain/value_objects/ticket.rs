use super::{Attachment, Caller};
use crate::{attachments::domain::AttachmentError, conversation::domain::ConversationId};
use nessa_auth::domain::OrganizationId;
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use std::fmt;
use subtle::ConstantTimeEq;

/// How long an unused ticket stays usable.
pub const TICKET_LIFETIME_MS: u64 = 5 * 60 * 1000;

/// When a ticket was issued and the last millisecond it may be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TicketLifetime {
    issued_at_ms: u64,
    expires_at_ms: u64,
}
impl TicketLifetime {
    /// A lifetime of [`TICKET_LIFETIME_MS`] starting at `now_ms`.
    pub fn starting(now_ms: u64) -> Result<Self, AttachmentError> {
        let expires_at_ms = now_ms
            .checked_add(TICKET_LIFETIME_MS)
            .ok_or(AttachmentError::Lifetime)?;
        Ok(Self {
            issued_at_ms: now_ms,
            expires_at_ms,
        })
    }
    /// Usable through `expires_at_ms` itself and refused after it, which is
    /// what the wire contract promises about that number. A clock that reads
    /// earlier than the issue time proves nothing either way, so it is usable.
    pub fn is_usable_at(&self, now_ms: u64) -> bool {
        now_ms <= self.expires_at_ms
    }
    pub fn issued_at_ms(&self) -> u64 {
        self.issued_at_ms
    }
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
}

/// SHA-256 of a ticket's secret. The secret itself is never kept.
///
/// Comparison is constant-time and there is no `PartialEq`, so a fingerprint
/// cannot be compared by accident with an early-exit equality.
#[derive(Clone, Copy)]
pub struct TicketFingerprint([u8; 32]);
impl TicketFingerprint {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn matches(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}
impl fmt::Debug for TicketFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TicketFingerprint(..)")
    }
}

/// How received bytes disagreed with the ticket they were sent under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadMismatch {
    Size,
    Digest,
}

/// Permission to upload exactly one described file into exactly one
/// conversation, for the caller it was issued to, until it expires.
///
/// Everything a finished upload will be attributed to is fixed here, when the
/// authenticated socket issues the ticket. The upload route knows nothing
/// about who is calling; it only learns what this value says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadTicket {
    organization_id: OrganizationId,
    conversation_id: ConversationId,
    attachment: Attachment,
    caller: Caller,
    lifetime: TicketLifetime,
}
impl UploadTicket {
    pub fn new(
        organization_id: OrganizationId,
        conversation_id: ConversationId,
        attachment: Attachment,
        caller: Caller,
        lifetime: TicketLifetime,
    ) -> Self {
        Self {
            organization_id,
            conversation_id,
            attachment,
            caller,
            lifetime,
        }
    }
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }
    /// The file the caller said it would upload.
    pub fn attachment(&self) -> &Attachment {
        &self.attachment
    }
    pub fn caller(&self) -> &Caller {
        &self.caller
    }
    pub fn lifetime(&self) -> TicketLifetime {
        self.lifetime
    }
    /// Whether `other` is this same request made again: the same caller, under
    /// the same action identifier, describing the same file for the same
    /// conversation. Only when it was issued may differ.
    pub fn repeats(&self, other: &Self) -> bool {
        self.organization_id == other.organization_id
            && self.conversation_id == other.conversation_id
            && self.attachment == other.attachment
            && self.caller == other.caller
    }
    /// Whether this many received bytes already rule the upload out, so a
    /// transfer can stop as soon as it runs long instead of at its end.
    pub fn is_exceeded_by(&self, received: u64) -> bool {
        received > self.attachment.size()
    }
    /// The whole transfer must be exactly the described file. Size is checked
    /// first: a wrong length makes the digest meaningless.
    pub fn check_received(&self, size: u64, digest: Sha256Digest) -> Result<(), UploadMismatch> {
        if size != self.attachment.size() {
            return Err(UploadMismatch::Size);
        }
        if digest != self.attachment.digest() {
            return Err(UploadMismatch::Digest);
        }
        Ok(())
    }
}
