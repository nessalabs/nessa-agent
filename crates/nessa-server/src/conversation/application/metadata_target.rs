//! Correlates metadata replies with the caller-owned target before ownership,
//! selection, creation audit or provider effects consume the returned record.

use super::{ConversationError, ConversationRepository};
use crate::conversation::domain::Conversation;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::sessions::StorageError;

/// Loads and create acknowledgements share this target relationship.
/// Existing creation actor/action/time remain historical, not this caller's
/// new intent; Created is correlated with its proposal before auditing.
/// `creation_metadata_target_refuses_foreign_acknowledgement_and_accepts_stored_retry`.
pub(crate) fn validate_target(
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
pub(crate) async fn load_conversation(
    repository: &dyn ConversationRepository,
    id: &ConversationId,
) -> Result<Option<Conversation>, ConversationError> {
    let record = repository.load(id).await?;
    if let Some(record) = record.as_ref() {
        validate_target(id, record)?;
    }
    Ok(record)
}
