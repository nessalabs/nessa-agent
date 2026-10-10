//! Files an environment publishes under a lease, kept for the conversation
//! (issue #701; `docs/design/runtime-architecture.md`, "Artifact channel
//! bounds and resumption").
//!
//! ```text
//! LeaseHold::artifacts ──▶ ArtifactOffer { file, bytes, answer }, one at a time
//!   over the lease's budget? ──▶ refused budgetExceeded, nothing read
//!   no room for its record? ──▶ refused budgetExceeded, nothing read
//!   name or type unreadable? ──▶ refused invalid, nothing read
//!   ConversationAttachments::keep_published(file, bytes)
//!     the conversation already holds it ──▶ alreadyHeld, nothing read
//!     else bytes pulled ──▶ size and digest checked ──▶ held under the lease
//!     interrupted, mismatched, or the lease ended ──▶ refused, no hold
//!   SessionManager::record_artifact ──▶ the conversation's record of it
//!   answer ──▶ the host lets go of its staged copy
//! ```
//!
//! Arrows are steps, in order. Nothing is kept from bytes not checked
//! against the digest the host published, and a hold is made only once
//! its bytes are; a record is made only for a file the conversation holds.
//! Every offer is answered, by its answer's drop if by nothing else.
use super::{ArtifactKept, ConversationAttachments, PublishedArtifact};
use nessa_auth::domain::OrganizationId;
use nessa_protocol::{
    conversation::domain::ConversationId,
    lease::{Collection, CollectionRefusal, StagedArtifact},
};
use nessa_sdk::application::agent_execution::{
    agents::Agent,
    permissions::ActionContext,
    sessions::{ArtifactName, ArtifactRecord, ArtifactRecordError, PublishedFile},
};
use nessa_sdk::domain::{
    agent_execution::leases::LeaseId,
    common::value_objects::{MediaType, Sha256Digest},
};
use std::{future::Future, pin::Pin, sync::Arc};

/// One file the host published under a lease, offered to be kept.
pub struct ArtifactOffer {
    /// What the host says the file is; nothing of it is checked yet.
    pub file: StagedArtifact,
    /// Its bytes, read only when they are pulled.
    pub bytes: Box<dyn ArtifactBytes>,
    /// Tells the host what became of it.
    pub answer: ArtifactAnswer,
}

/// What [`ArtifactBytes::next`] answers.
pub type ArtifactChunk<'a> =
    Pin<Box<dyn Future<Output = Result<Option<Vec<u8>>, ArtifactReadFailure>> + Send + 'a>>;

/// A published file's bytes, pulled in order.
pub trait ArtifactBytes: Send {
    /// The next chunk, or `None` once every byte the host published was
    /// read. Nothing is read until this is first asked.
    fn next(&mut self) -> ArtifactChunk<'_>;
}

/// Why a published file's bytes stopped before their end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactReadFailure {
    /// The channel to the host failed past what resuming covers.
    Unavailable,
    /// The lease is no longer held there.
    LeaseEnded,
    /// The host's file is not what it published: gone, or another size.
    Changed,
}

impl ArtifactReadFailure {
    /// What the host is told of a file whose read failed this way.
    pub fn refusal(self) -> CollectionRefusal {
        match self {
            Self::Unavailable => CollectionRefusal::ChannelUnavailable,
            Self::LeaseEnded => CollectionRefusal::LeaseEnded,
            Self::Changed => CollectionRefusal::Mismatch,
        }
    }
}

/// The one answer the host is given about an offer. Dropped unanswered, it
/// answers that the file was not kept, so the host never waits on nobody.
pub struct ArtifactAnswer {
    send: Option<Box<dyn FnOnce(Collection) + Send>>,
}

impl ArtifactAnswer {
    pub fn new(send: impl FnOnce(Collection) + Send + 'static) -> Self {
        Self {
            send: Some(Box::new(send)),
        }
    }

    pub fn answer(mut self, outcome: Collection) {
        if let Some(send) = self.send.take() {
            send(outcome);
        }
    }
}

impl Drop for ArtifactAnswer {
    fn drop(&mut self) {
        if let Some(send) = self.send.take() {
            send(Collection::Refused {
                reason: CollectionRefusal::NotKept,
            });
        }
    }
}

/// Most times a published file's read is resumed on a new channel after
/// its channel failed, from the offset it reached. Row
/// `artifact.read_resumes` in `docs/limits.md`.
pub const ARTIFACT_READ_RESUMES: u32 = 3;

/// Most one lease may publish and have kept: past either, a file is
/// refused unread. Rows `artifact.environment_files` and `artifact.environment_bytes`
/// in `docs/limits.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactBudget {
    pub(crate) files: u32,
    pub(crate) bytes: u64,
}

impl ArtifactBudget {
    pub(crate) const LEASE: Self = Self {
        files: 64,
        bytes: 1024 * 1024 * 1024,
    };
}

/// What one lease's files are kept for: the conversation, and the lease
/// with the person who asked for its work.
pub(crate) struct ArtifactCollector {
    pub(crate) organization_id: OrganizationId,
    pub(crate) conversation_id: ConversationId,
    pub(crate) lease: LeaseId,
    pub(crate) issuer: ActionContext,
    pub(crate) attachments: Option<Arc<dyn ConversationAttachments>>,
    pub(crate) budget: ArtifactBudget,
    /// What this lease's files kept so far took: files, bytes.
    spent: (u32, u64),
}

impl ArtifactCollector {
    pub(crate) fn new(
        organization_id: OrganizationId,
        conversation_id: ConversationId,
        lease: LeaseId,
        issuer: ActionContext,
        attachments: Option<Arc<dyn ConversationAttachments>>,
        budget: ArtifactBudget,
    ) -> Self {
        Self {
            organization_id,
            conversation_id,
            lease,
            issuer,
            attachments,
            budget,
            spent: (0, 0),
        }
    }

    /// Keep `offer` for the conversation `agent` runs, and answer it.
    pub(crate) async fn collect(&mut self, offer: ArtifactOffer, agent: &Agent) {
        let ArtifactOffer {
            file,
            bytes,
            answer,
        } = offer;
        let outcome = match self.keep(file, bytes, agent).await {
            Ok(ArtifactKept::Held) => Collection::Held,
            Ok(ArtifactKept::AlreadyHeld) => Collection::AlreadyHeld,
            Err(reason) => Collection::Refused { reason },
        };
        answer.answer(outcome);
    }

    async fn keep(
        &mut self,
        file: StagedArtifact,
        bytes: Box<dyn ArtifactBytes>,
        agent: &Agent,
    ) -> Result<ArtifactKept, CollectionRefusal> {
        let (files, spent) = self.spent;
        let over =
            files >= self.budget.files || spent.saturating_add(file.size) > self.budget.bytes;
        if over {
            return Err(CollectionRefusal::BudgetExceeded);
        }
        let attachments = self
            .attachments
            .clone()
            .ok_or(CollectionRefusal::ChannelUnavailable)?;
        let name = ArtifactName::new(file.name.clone()).map_err(|_| CollectionRefusal::Invalid)?;
        let published = Sha256Digest::parse(&format!("sha256:{}", file.digest))
            .ok()
            .zip(MediaType::parse(&file.media_type).ok())
            .and_then(|(digest, media_type)| PublishedFile::new(digest, media_type, file.size).ok())
            .ok_or(CollectionRefusal::Invalid)?;
        let manager = agent.session_manager();
        // Never a file held that the conversation could not record.
        match manager.artifact_room().await {
            Some(0) => return Err(CollectionRefusal::BudgetExceeded),
            Some(_) => {}
            None => return Err(CollectionRefusal::LeaseEnded),
        }
        let size = file.size;
        let kept = attachments
            .keep_published(PublishedArtifact {
                organization_id: self.organization_id.clone(),
                conversation_id: self.conversation_id.clone(),
                lease: self.lease.as_str().to_owned(),
                requested_by: self.issuer.clone(),
                file,
                bytes,
            })
            .await?;
        if kept == ArtifactKept::Held {
            self.spent = (files.saturating_add(1), spent.saturating_add(size));
        }
        let record = |turn| ArtifactRecord {
            lease: self.lease.clone(),
            turn,
            name: name.clone(),
            file: published.clone(),
            actor: self.issuer.clone(),
        };
        let mut recorded = manager
            .record_artifact(record(agent.active_execution_id()))
            .await;
        // A turn the conversation has not yet taken in: the file is still
        // the lease's, recorded without one.
        if recorded == Err(ArtifactRecordError::UnknownTurn) {
            recorded = manager.record_artifact(record(None)).await;
        }
        match recorded {
            Ok(()) => Ok(kept),
            // Retained, and written with the next save, as a lease record
            // is: the record stands.
            Err(ArtifactRecordError::Storage(error)) => {
                tracing::error!(conversation_id = %self.conversation_id, %error, "an artifact record was retained but not yet saved");
                Ok(kept)
            }
            // The file stays held until the conversation lets go of its
            // files; the host is told it was not kept for this lease.
            Err(ArtifactRecordError::Full) => Err(CollectionRefusal::BudgetExceeded),
            Err(ArtifactRecordError::NotThisLease) => Err(CollectionRefusal::LeaseEnded),
            Err(_) => Err(CollectionRefusal::NotKept),
        }
    }
}
