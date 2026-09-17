use super::ConversationError;
use crate::conversation::domain::{Conversation, ConversationId};
use nessa_auth::domain::{OrganizationId, PrincipalId};
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
