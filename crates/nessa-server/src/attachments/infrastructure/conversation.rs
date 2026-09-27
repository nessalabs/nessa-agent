//! The two places this context meets the conversation context, each through
//! the other side's own port: conversations ask what they hold and let go on
//! close, and an upload asks who owns a conversation without opening an agent.
use crate::{
    attachments::{
        application::{
            AttachmentService, ConversationOwnership, Ownership, OwnershipUnavailable, PortFuture,
            ReleaseCause, ReleaseError, ReleaseRequest,
        },
        domain::Attachment,
    },
    conversation::{
        application::{
            AttachmentRelease, AttachmentReleaseCause, ConversationAttachments, ConversationError,
            ConversationFuture, ConversationRepository,
        },
        domain::{ConversationId, ConversationRefusal},
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::agent_execution::prompts::ImageReference;
use std::sync::Arc;

/// What a conversation holds, answered by the attachment service.
pub struct ConversationHolds {
    service: AttachmentService,
}
impl ConversationHolds {
    pub fn new(service: AttachmentService) -> Self {
        Self { service }
    }
}
impl ConversationAttachments for ConversationHolds {
    fn holds<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        image: &'a ImageReference,
    ) -> ConversationFuture<'a, bool> {
        Box::pin(async move {
            // A message names what the conversation keeps, which for an image
            // is the normalized file and not the one that was uploaded.
            self.service
                .holds(
                    organization_id,
                    conversation_id,
                    &Attachment::of_image(image),
                )
                .await
                .map_err(|_| ConversationError::Unavailable)
        })
    }
    fn release(&self, release: AttachmentRelease) -> ConversationFuture<'_, ()> {
        Box::pin(async move {
            let cause = match release.cause {
                AttachmentReleaseCause::ConversationClosed => ReleaseCause::ConversationClosed,
                AttachmentReleaseCause::ConversationDeleted => ReleaseCause::ConversationDeleted,
            };
            self.service
                .release(ReleaseRequest {
                    organization_id: release.organization_id,
                    conversation_id: release.conversation_id,
                    cause,
                    principal_id: release.initiator_principal_id,
                    surface_id: release.initiator_surface_id,
                    correlation_id: release.correlation_id,
                })
                .await
                // Both facts cross the boundary: what is still in place and
                // what happened without evidence are different failures, and a
                // close that met both reports both.
                .map_err(|error| match error {
                    ReleaseError::Unattributable => ConversationError::InvalidInput,
                    ReleaseError::Incomplete {
                        storage_failures,
                        audit_failures,
                    } => ConversationError::AttachmentCleanup {
                        storage_failures,
                        audit_failures,
                    },
                })
        })
    }
}

/// Conversation ownership read from the conversation context's own repository.
pub struct RepositoryOwnership {
    conversations: Arc<dyn ConversationRepository>,
}
impl RepositoryOwnership {
    pub fn new(conversations: Arc<dyn ConversationRepository>) -> Self {
        Self { conversations }
    }
}
impl ConversationOwnership for RepositoryOwnership {
    fn owns<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        principal_id: &'a PrincipalId,
        conversation_id: &'a ConversationId,
    ) -> PortFuture<'a, Ownership, OwnershipUnavailable> {
        Box::pin(async move {
            // The conversation's own rule decides: organization and owner must
            // both agree, knowing an identifier grants nothing, and a deleted
            // conversation is refused — to its owner, as deleted.
            let found = self
                .conversations
                .load(conversation_id)
                .await
                .map_err(|_| OwnershipUnavailable)?;
            Ok(match found {
                None => Ownership::NotFound,
                Some(conversation) => {
                    match conversation.check_access(organization_id, principal_id) {
                        Ok(()) => Ownership::Owned,
                        Err(ConversationRefusal::NotFound) => Ownership::NotFound,
                        Err(ConversationRefusal::Deleted) => Ownership::Deleted,
                    }
                }
            })
        })
    }
}
