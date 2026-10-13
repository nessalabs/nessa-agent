//! Notes left in a conversation (`conversation.message`, ADR 0011 phase B;
//! slice J3 of #707): what a Comment grant, or the owner, leaves for the
//! conversation without starting or steering a turn, and where the sender's
//! own note stands (`conversation.messageStatus`).
//!
//! ```text
//! leave_note(conversation, caller, author, text, delivery)
//!   ──▶ the caller's conversation, not deleted ── else not_found / deleted
//!   ──▶ its agent, opened as a read opens it
//!   ──▶ SessionManager::record_note ──▶ the receipt, or the same request's first one
//!         another note under the request ──▶ idempotency_conflict
//!         full, or the author past its rate ──▶ message_quota_exceeded
//! note_status(conversation, caller)
//!   ──▶ the caller's conversation ──▶ its committed records ──▶ the caller's note, if saved
//! ```
//!
//! The rules are the SDK's ([`NoteRecord`]): the conversation's records are
//! the one place a note, its retry and its quota are decided, not a ledger
//! here. Who may leave one is the socket's admission
//! (`product::read_access::admit_request`), which hands this the author.
use super::{conversation_session, load_conversation, ConversationService};
use crate::conversation::application::{ConversationCaller, ConversationError};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::sessions::{
    NoteDelivery, NoteRecord, NoteRecordError, NoteRequest, NoteText, StorageError,
};
use nessa_sdk::domain::agent_execution::executions::Initiator;

impl ConversationService {
    /// Leave a note in conversation `id`, from `caller` under `author`, the
    /// authority it was admitted under. A note starts no turn and steers
    /// none (row J21). The same request again answers its first note
    /// (row J18).
    ///
    /// # Errors
    /// `InvalidInput` for text outside [`NoteText`]'s bounds;
    /// `NotFound` or `Deleted` for a conversation that is not the caller's
    /// or was deleted; `IdempotencyConflict` for another note under the same
    /// request; `MessageQuotaExceeded` past the conversation's bound or the
    /// author's rate (row J19); whatever opening the conversation answers.
    pub async fn leave_note(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        author: Initiator,
        text: String,
        delivery: NoteDelivery,
    ) -> Result<NoteRecord, ConversationError> {
        let text = NoteText::new(text).map_err(|_| ConversationError::InvalidInput)?;
        let actor = caller.actor()?;
        let _admission = self.admit().await?;
        let live = self.resolve(&id, &caller).await?;
        live.agent
            .session_manager()
            .record_note(NoteRequest {
                text,
                delivery,
                actor,
                author,
            })
            .await
            .map_err(|error| match error {
                NoteRecordError::Conflict => ConversationError::IdempotencyConflict,
                NoteRecordError::Full | NoteRecordError::RateLimited => {
                    ConversationError::MessageQuotaExceeded
                }
                NoteRecordError::NotLoaded => ConversationError::Unavailable,
                NoteRecordError::Storage(error) => ConversationError::Storage(error),
            })
    }

    /// The note `caller`'s principal left in conversation `id` under its
    /// request, as the conversation's committed records hold it: `None` when
    /// it left none, or none was saved (row J20). Nothing is opened.
    ///
    /// # Errors
    /// `NotFound` or `Deleted` as for [`Self::leave_note`]; a storage
    /// failure reading the records.
    pub async fn note_status(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
    ) -> Result<Option<NoteRecord>, ConversationError> {
        caller.actor()?;
        let _admission = self.admit().await?;
        let record = load_conversation(self.inner.metadata.as_ref(), &id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        record.check_access(&caller.organization_id, &caller.principal_id)?;
        let session_id = conversation_session(&id);
        let committed = self
            .inner
            .storage
            .read_committed(session_id.clone())
            .await?;
        if committed
            .as_ref()
            .is_some_and(|committed| committed.id() != &session_id)
        {
            return Err(ConversationError::Storage(StorageError::IdentityMismatch));
        }
        Ok(committed
            .as_ref()
            .and_then(|committed| committed.snapshot())
            .and_then(|snapshot| snapshot.note(caller.principal_id.as_str(), &caller.action_id))
            .cloned())
    }
}
