use super::ConversationError;
use crate::conversation::domain::{Conversation, ConversationId};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::agent_execution::prompts::ImageReference;
use std::{future::Future, pin::Pin};

pub type ConversationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ConversationError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationCreationDisposition {
    Created,
    Existing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationCreation {
    pub conversation: Conversation,
    pub disposition: ConversationCreationDisposition,
}

/// Immutable evidence for one acknowledged create or reopen request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationCreationAuditRecord {
    pub conversation_id: ConversationId,
    pub organization_id: OrganizationId,
    pub owner_id: PrincipalId,
    pub before: ConversationOwnershipState,
    pub after: ConversationOwnershipState,
    pub cause: ConversationCreationCause,
    pub initiator_principal_id: PrincipalId,
    pub initiator_surface_id: String,
    pub correlation_id: String,
    pub requested_at_ms: u64,
    pub observed_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationOwnershipState {
    Absent,
    Owned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationCreationCause {
    CallerRequested,
    IdempotentReopen,
}

/// Commits creation evidence before the application reports success.
/// Caller-requested creation records are idempotent by conversation identity so
/// an interrupted first audit delivery can be safely retried from stored evidence.
pub trait ConversationCreationAudit: Send + Sync {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()>;
}
/// Stores only conversation ownership. The SDK stores execution history separately.
pub trait ConversationRepository: Send + Sync {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<Conversation>>;
    /// Create once, or return the existing owner without changing it.
    fn create(&self, conversation: Conversation) -> ConversationFuture<'_, ConversationCreation>;
}

/// An image as a caller submitted it, before any of it has been checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedImage {
    pub digest: String,
    pub media_type: String,
    pub size: u64,
}

/// Why a conversation let go of the files uploaded into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentReleaseCause {
    /// The verified caller closed the conversation.
    ConversationClosed,
}

/// A verified caller's release of one conversation's uploaded files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentRelease {
    pub organization_id: OrganizationId,
    pub conversation_id: ConversationId,
    pub cause: AttachmentReleaseCause,
    pub initiator_principal_id: PrincipalId,
    pub initiator_surface_id: String,
    pub correlation_id: String,
}

/// What a conversation needs from wherever uploaded files are kept.
///
/// A message refers to images by digest. Before accepting one, the service asks
/// whether *this* conversation holds exactly those bytes, so a digest learned
/// elsewhere cannot be used to read another owner's upload.
pub trait ConversationAttachments: Send + Sync {
    /// Whether the conversation holds an upload whose digest, media type, and
    /// size all agree with `image`.
    fn holds<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        image: &'a ImageReference,
    ) -> ConversationFuture<'a, bool>;
    /// Let go of everything the conversation holds. The implementation records
    /// the release with its cause and initiator; an `Err` reports that some of
    /// it, or its evidence, could not be completed after every part was tried.
    fn release(&self, release: AttachmentRelease) -> ConversationFuture<'_, ()>;
}
