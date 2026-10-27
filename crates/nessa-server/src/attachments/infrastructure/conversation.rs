//! The two places this context meets the conversation context, each through
//! the other side's own port: conversations ask what they hold, keep what
//! an environment published, read back a held file for a fetch, and let go
//! on close; and an upload asks who owns a conversation without opening an
//! agent.
use crate::{
    attachments::{
        application::{
            AttachmentService, ConversationOwnership, HeldReadError, HeldReads, Ownership,
            OwnershipUnavailable, PortFuture, PublishKept, PublishedFile, ReleaseCause,
            ReleaseError, ReleaseRequest, UploadBody, UploadError, UploadInterrupted,
            UploadRejection,
        },
        domain::{Attachment, Caller, MediaType},
    },
    conversation::{
        application::{
            AccessAction, ArtifactBytes, ArtifactKept, ArtifactReadFailure, AttachmentRelease,
            AttachmentReleaseCause, ConversationAccessPolicyEvaluator, ConversationAttachments,
            ConversationError, ConversationFuture, ConversationRepository, HeldRange,
            HeldRangeError, OwnerAccess, PublishedArtifact,
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

/// What a conversation holds, answered by the attachment service, and its
/// held files read back by range as a surface reads them.
pub struct ConversationHolds {
    service: AttachmentService,
    reads: HeldReads,
}
impl ConversationHolds {
    pub fn new(service: AttachmentService, reads: HeldReads) -> Self {
        Self { service, reads }
    }
}
impl ConversationAttachments for ConversationHolds {
    fn holds_file<'a>(
        &'a self,
        organization: &'a OrganizationId,
        conversation: &'a ConversationId,
        file: &'a nessa_sdk::domain::agent_execution::prompts::UploadedFileReference,
    ) -> ConversationFuture<'a, bool> {
        Box::pin(async move {
            let media = MediaType::parse(file.media_type().as_str())
                .map_err(|_| ConversationError::InvalidInput)?;
            let attachment = Attachment::new(file.digest(), media, file.size())
                .map_err(|_| ConversationError::InvalidInput)?;
            self.service
                .holds(organization, conversation, &attachment)
                .await
                .map_err(|_| ConversationError::Unavailable)
        })
    }

    fn offers_images(&self) -> bool {
        self.service.offers_images()
    }
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
    fn read_held(
        &self,
        range: HeldRange,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, HeldRangeError>> + Send + '_>> {
        Box::pin(async move {
            // The file a fetch names is the one its record names: one
            // attachment, the three facts together.
            let file = MediaType::parse(range.file.media_type().as_str())
                .ok()
                .and_then(|media_type| {
                    Attachment::new(*range.file.digest(), media_type, range.file.size()).ok()
                })
                .ok_or(HeldRangeError::NotHeld)?;
            // Someone no record could name owns nothing here.
            let principal =
                PrincipalId::new(range.principal_id).map_err(|_| HeldRangeError::NotHeld)?;
            let max_bytes = range.max_bytes.min(HeldReads::MAX_RANGE_BYTES);
            self.reads
                .read(
                    &range.organization_id,
                    &principal,
                    &range.conversation_id,
                    &file,
                    range.offset,
                    max_bytes,
                )
                .await
                .map(|bytes| bytes.as_bytes().to_vec())
                .map_err(|error| match error {
                    // Not the person's conversation, or not there: it holds
                    // nothing for them.
                    HeldReadError::NotHeld
                    | HeldReadError::ConversationNotFound
                    | HeldReadError::ConversationDeleted => HeldRangeError::NotHeld,
                    HeldReadError::Busy => HeldRangeError::Busy,
                    HeldReadError::InvalidRequest
                    | HeldReadError::Unavailable
                    | HeldReadError::OwnershipUnavailable => HeldRangeError::Unavailable,
                })
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

/// Conversation ownership read from the conversation context's own
/// repository and decided by its access policy, for one action: uploading
/// into a conversation acts on it (`conversation.act`), and reading back a
/// file it holds is `artifact.read`.
pub struct RepositoryOwnership {
    conversations: Arc<dyn ConversationRepository>,
    policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    action: AccessAction,
}
impl RepositoryOwnership {
    pub fn new(
        conversations: Arc<dyn ConversationRepository>,
        policy: Arc<dyn ConversationAccessPolicyEvaluator>,
        action: AccessAction,
    ) -> Self {
        Self {
            conversations,
            policy,
            action,
        }
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
            // The conversation's access policy decides: knowing an
            // identifier grants nothing, and a deleted conversation is
            // refused — as deleted only to whom the policy lets locate it.
            let found = self
                .conversations
                .load(conversation_id)
                .await
                .map_err(|_| OwnershipUnavailable)?;
            let Some(conversation) = found else {
                return Ok(Ownership::NotFound);
            };
            match OwnerAccess::new(self.policy.as_ref()).admit_to(
                self.action,
                &conversation,
                organization_id,
                principal_id,
            ) {
                Ok(()) => Ok(Ownership::Owned),
                Err(ConversationRefusal::NotFound) => Ok(Ownership::NotFound),
                Err(ConversationRefusal::Deleted) => Ok(Ownership::Deleted),
                Err(ConversationRefusal::Unavailable) => Err(OwnershipUnavailable),
            }
        })
    }
}
