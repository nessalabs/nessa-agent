//! Immutable current metadata returned to passive catalogue consumers.
//! Product value constructors own field validity; this read model grants no
//! permission and contains neither live controls nor transcript state.

use crate::{
    agents::domain::AgentId,
    conversation::domain::{
        ConversationApprovalMode, ConversationId, ConversationModelId, ConversationSummary,
        LATEST_TIME_MS,
    },
};

/// A current catalogue entry's validated product metadata, separate from its
/// core revision/deletion descriptor. Saving both remains the cache's job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueMetadata {
    id: ConversationId,
    created_at_ms: u64,
    agent: Option<AgentId>,
    model: ConversationModelId,
    approval_mode: ConversationApprovalMode,
    summary: Option<ConversationSummary>,
}

/// Invalid grouping of otherwise typed metadata fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogueMetadataError {
    /// Creation time exceeds the product's published JSON time ceiling.
    CreationTime,
}

impl CatalogueMetadata {
    /// Constructs a passive snapshot from validated product values.
    /// `created_at_ms` is the creation request observation in Unix milliseconds.
    /// Inputs are owned and retained immutably; this operation performs no I/O.
    ///
    /// # Errors
    /// Returns [`CatalogueMetadataError::CreationTime`] above [`LATEST_TIME_MS`].
    pub fn new(
        id: ConversationId,
        created_at_ms: u64,
        agent: Option<AgentId>,
        model: ConversationModelId,
        approval_mode: ConversationApprovalMode,
        summary: Option<ConversationSummary>,
    ) -> Result<Self, CatalogueMetadataError> {
        if created_at_ms > LATEST_TIME_MS {
            return Err(CatalogueMetadataError::CreationTime);
        }
        Ok(Self {
            id,
            created_at_ms,
            agent,
            model,
            approval_mode,
            summary,
        })
    }

    pub fn id(&self) -> &ConversationId {
        &self.id
    }
    pub fn created_at_ms(&self) -> u64 {
        self.created_at_ms
    }
    pub fn agent(&self) -> Option<AgentId> {
        self.agent
    }
    pub fn model(&self) -> &ConversationModelId {
        &self.model
    }
    pub fn approval_mode(&self) -> ConversationApprovalMode {
        self.approval_mode
    }
    pub fn summary(&self) -> Option<&ConversationSummary> {
        self.summary.as_ref()
    }
}
