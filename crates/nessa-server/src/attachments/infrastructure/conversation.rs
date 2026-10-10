//! The two places this context meets the conversation context, each through
//! the other side's own port: conversations ask what they hold and let go on
//! close, and an upload asks who owns a conversation without opening an agent.
use crate::{
    attachments::{
        application::{
            AttachmentService, ConversationOwnership, Ownership, OwnershipUnavailable, PortFuture,
            PublishKept, PublishedFile, ReleaseCause, ReleaseError, ReleaseRequest, UploadBody,
            UploadError, UploadInterrupted, UploadRejection,
        },
        domain::{Attachment, Caller, MediaType},
    },
    conversation::{
        application::{
            ArtifactBytes, ArtifactKept, ArtifactReadFailure, AttachmentRelease,
            AttachmentReleaseCause, ConversationAttachments, ConversationError, ConversationFuture,
            ConversationRepository, PublishedArtifact,
        },
        domain::ConversationRefusal,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::lease::CollectionRefusal;
use nessa_sdk::domain::{
    agent_execution::prompts::ImageReference, common::value_objects::Sha256Digest,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
};

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
    fn keep_published(
        &self,
        published: PublishedArtifact,
    ) -> Pin<Box<dyn Future<Output = Result<ArtifactKept, CollectionRefusal>> + Send + '_>> {
        Box::pin(async move {
            let PublishedArtifact {
                organization_id,
                conversation_id,
                lease,
                requested_by,
                file,
                bytes,
                record,
            } = published;
            // What the host said, read as this context reads any file: a
            // name for what it is, never trusted for what its bytes are.
            let attachment = Sha256Digest::parse(&format!("sha256:{}", file.digest))
                .ok()
                .zip(MediaType::parse(&file.media_type).ok())
                .and_then(|(digest, media_type)| {
                    Attachment::new(digest, media_type, file.size).ok()
                })
                .ok_or(CollectionRefusal::Invalid)?;
            let caller = PrincipalId::new(requested_by.principal_id())
                .ok()
                .and_then(|principal| {
                    Caller::new(
                        principal,
                        requested_by.surface_id(),
                        requested_by.request_id(),
                    )
                    .ok()
                })
                .ok_or(CollectionRefusal::NotKept)?;
            let published =
                PublishedFile::new(organization_id, conversation_id, attachment, caller, &lease)
                    .ok_or(CollectionRefusal::Invalid)?;
            let failure = Arc::new(Mutex::new(None));
            let body = Box::new(PulledBytes {
                bytes,
                failure: failure.clone(),
            });
            match self.service.keep_published(published, body, record).await {
                Ok(PublishKept::Held(_)) => Ok(ArtifactKept::Held),
                Ok(PublishKept::AlreadyHeld(_)) => Ok(ArtifactKept::AlreadyHeld),
                Err(error) => {
                    let read = *failure.lock().unwrap_or_else(PoisonError::into_inner);
                    Err(refusal(error, read))
                }
            }
        })
    }
}

/// Why the host is told a published file was not kept: a read that failed
/// says how, a file that differs from what was published is a mismatch,
/// and anything on this side is not kept.
fn refusal(error: UploadError, read: Option<ArtifactReadFailure>) -> CollectionRefusal {
    match error {
        UploadError::Rejected { reason, .. } => match reason {
            UploadRejection::SizeMismatch | UploadRejection::DigestMismatch => {
                CollectionRefusal::Mismatch
            }
            UploadRejection::UploadInterrupted => read.map_or(
                CollectionRefusal::ChannelUnavailable,
                ArtifactReadFailure::refusal,
            ),
            UploadRejection::UploadTimeout => CollectionRefusal::ChannelUnavailable,
            _ => CollectionRefusal::NotKept,
        },
        _ => CollectionRefusal::NotKept,
    }
}

/// A published file's bytes as an upload's body, keeping why a read failed.
struct PulledBytes {
    bytes: Box<dyn ArtifactBytes>,
    failure: Arc<Mutex<Option<ArtifactReadFailure>>>,
}
impl UploadBody for PulledBytes {
    fn next(&mut self) -> PortFuture<'_, Option<Vec<u8>>, UploadInterrupted> {
        Box::pin(async move {
            self.bytes.next().await.map_err(|read| {
                *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(read);
                UploadInterrupted
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
