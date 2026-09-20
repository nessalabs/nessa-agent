//! The two places this context meets the conversation context, each through
//! the other side's own port: conversations ask what they hold and let go on
//! close, and an upload asks who owns a conversation without opening an agent.
use crate::{
    attachments::{
        application::{
            AttachmentService, ConversationOwnership, OwnershipUnavailable, PortFuture,
            ReleaseCause, ReleaseRequest,
        },
        domain::Attachment,
    },
    conversation::{
        application::{
            AttachmentRelease, AttachmentReleaseCause, ConversationAttachments, ConversationError,
            ConversationFuture, ConversationRepository,
        },
        domain::ConversationId,
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
                // One error must stand for both kinds. Files still in place are
                // the failure a retry can act on, so they are named first; the
                // service has already logged both counts.
                .map_err(|error| {
                    if error.storage_failures != 0 {
                        ConversationError::Unavailable
                    } else {
                        ConversationError::Audit
                    }
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
    ) -> PortFuture<'a, bool, OwnershipUnavailable> {
        Box::pin(async move {
            // The conversation's own rule decides: organization and owner must
            // both agree, and knowing an identifier grants nothing.
            self.conversations
                .load(conversation_id)
                .await
                .map(|found| {
                    found.is_some_and(|conversation| {
                        conversation.allows(organization_id, principal_id)
                    })
                })
                .map_err(|_| OwnershipUnavailable)
        })
    }
}
