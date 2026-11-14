//! Correlates metadata replies with the caller-owned target before ownership,
//! selection, creation audit or provider effects consume the returned record.

use super::{
    ConversationAdmissionSnapshot, ConversationAdmissionToken, ConversationError,
    ConversationRepository,
};
use crate::domain::Conversation;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::sessions::StorageError;
use std::sync::Arc;

/// Loads and create acknowledgements share this target relationship.
/// Existing creation actor/action/time remain historical, not this caller's
/// new intent; Created is correlated with its proposal before auditing.
/// `creation_metadata_target_refuses_foreign_acknowledgement_and_accepts_stored_retry`.
pub fn validate_target(
    id: &ConversationId,
    record: &Conversation,
) -> Result<(), ConversationError> {
    if record.id() != id {
        return Err(ConversationError::Storage(StorageError::IdentityMismatch));
    }
    Ok(())
}

/// Every application metadata load crosses the same boundary, including
/// rereads after awaits (`metadata_query_target_fences_effect_entry_paths`).
pub async fn load_conversation(
    repository: &dyn ConversationRepository,
    id: &ConversationId,
) -> Result<Option<Conversation>, ConversationError> {
    let record = repository.load(id).await?;
    if let Some(record) = record.as_ref() {
        validate_target(id, record)?;
    }
    Ok(record)
}

/// Metadata read dependency whose replies retain the requested identity.
/// The repository stays private so product review notices cannot bypass
/// `load_conversation` (`a_review_notice_refuses_mismatched_metadata_before_policy_or_publication`).
#[derive(Clone)]
pub struct ConversationMetadataReader {
    repository: Arc<dyn ConversationRepository>,
}

impl ConversationMetadataReader {
    pub fn new(repository: Arc<dyn ConversationRepository>) -> Self {
        Self { repository }
    }

    /// Conversation-only projection of [`Self::load_admission`].
    ///
    /// Library callers use [`load_conversation`]. Integration tests that already
    /// hold the reader call this method; that use is outside the library crate.
    #[allow(dead_code)]
    pub async fn load(
        &self,
        id: &ConversationId,
    ) -> Result<Option<Conversation>, ConversationError> {
        Ok(self
            .load_admission(id)
            .await?
            .map(ConversationAdmissionSnapshot::into_conversation))
    }

    /// Metadata and its admission token from one repository read.
    pub async fn load_admission(
        &self,
        id: &ConversationId,
    ) -> Result<Option<ConversationAdmissionSnapshot>, ConversationError> {
        let snapshot = self.repository.load_admission_snapshot(id).await?;
        if let Some(snapshot) = snapshot.as_ref() {
            validate_target(id, snapshot.conversation())?;
            if snapshot.token().conversation_id() != id {
                return Err(ConversationError::Storage(StorageError::IdentityMismatch));
            }
        }
        Ok(snapshot)
    }

    /// Whether `token` still names the row captured with it.
    pub async fn admission_current(
        &self,
        token: &ConversationAdmissionToken,
    ) -> Result<bool, ConversationError> {
        let fresh = self.load_admission(token.conversation_id()).await?;
        Ok(fresh
            .as_ref()
            .is_some_and(|snapshot| snapshot.token() == token))
    }
}
