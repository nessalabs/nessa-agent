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
//!   ──▶ the caller's conversation ──▶ its committed records, read on to their head
//!   ──▶ the caller's note, if saved
//! ```
//!
//! The rules are the SDK's ([`NoteRecord`]): the conversation's records are
//! the one place a note, its retry and its quota are decided, not a ledger
//! here. Who may leave one is the socket's admission
//! (`product::read_access::admit_request`), which hands this the author.
use super::{conversation_session, load_conversation, ConversationService, LiveConversation};
use crate::conversation::application::{
    note_candidates, ConversationCaller, ConversationError, ShareGrants,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::sessions::{
    CommittedSession, NoteCandidates, NoteDelivery, NoteRecord, NoteRecordError, NoteRequest,
    NoteState, NoteText, StorageError,
};
use nessa_sdk::domain::agent_execution::executions::Initiator;
use std::sync::Arc;

impl ConversationService {
    /// Bind the share grants a waiting note's author is checked against
    /// before a turn takes it (row J23). A second bind is ignored.
    pub fn bind_note_authors(&self, grants: Arc<dyn ShareGrants>) {
        let _ = self.inner.note_authors.set(grants);
    }

    /// The verdict on each note waiting in `live`'s committed records for a
    /// turn ([`note_candidates`]), asked of the conversation as it is stored
    /// now. Without bound grants, only the owner's notes are allowed and a
    /// grant's wait; the records unreadable, none is judged and every note
    /// waits.
    pub(super) async fn note_candidates(
        &self,
        id: &ConversationId,
        live: &LiveConversation,
    ) -> NoteCandidates {
        let Some(snapshot) = live.agent.session_manager().snapshot().await else {
            return NoteCandidates::default();
        };
        let waiting: Vec<NoteRecord> = snapshot
            .notes
            .iter()
            .filter(|note| snapshot.note_state(note) == NoteState::Pending)
            .cloned()
            .collect();
        let owners_only = || NoteCandidates {
            authorized: waiting
                .iter()
                .filter(|note| note.author == Initiator::Owner)
                .map(|note| note.id.clone())
                .collect(),
            revoked: Vec::new(),
        };
        let Some(grants) = self.inner.note_authors.get() else {
            return owners_only();
        };
        note_candidates(
            self.inner.access_policy.as_ref(),
            self.inner.metadata.as_ref(),
            grants.as_ref(),
            id,
            &waiting,
        )
        .await
    }

    /// Leave a note in conversation `id`, from `caller` under `author`, the
    /// authority it was admitted under, and where it stands once saved. A
    /// note starts no turn and steers none (row J21): a `next_turn` note
    /// waits for the next turn a person starts (row J23). The same request
    /// again answers its first note, where it stands now (row J18).
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
    ) -> Result<(NoteRecord, NoteState), ConversationError> {
        let text = NoteText::new(text).map_err(|_| ConversationError::InvalidInput)?;
        let actor = caller.actor()?;
        let _admission = self.admit().await?;
        let live = self.resolve(&id, &caller).await?;
        let manager = live.agent.session_manager();
        let record = manager
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
            })?;
        // Saved before the answer, so the committed records hold it.
        let state = manager
            .snapshot()
            .await
            .map(|snapshot| snapshot.note_state(&record))
            .ok_or(ConversationError::Unavailable)?;
        Ok((record, state))
    }

    /// The note `caller`'s principal left in conversation `id` under its
    /// request, and where it stands, as the conversation's committed records
    /// hold them: `None` when it left none, or none was saved (row J20).
    /// Nothing is opened.
    ///
    /// # Errors
    /// `NotFound` or `Deleted` as for [`Self::leave_note`]; a storage
    /// failure reading the records.
    pub async fn note_status(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
    ) -> Result<Option<(NoteRecord, NoteState)>, ConversationError> {
        caller.actor()?;
        let _admission = self.admit().await?;
        let record = load_conversation(self.inner.metadata.as_ref(), &id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, &caller)?;
        let session_id = conversation_session(&id);
        let found = |committed: &Option<CommittedSession>| {
            committed
                .as_ref()
                .and_then(CommittedSession::snapshot)
                .and_then(|snapshot| {
                    snapshot
                        .note(caller.principal_id.as_str(), &caller.action_id)
                        .map(|note| (note.clone(), snapshot.note_state(note)))
                })
        };
        // One committed read takes a bounded number of records, and a long
        // history needs several: a note past where this one stopped is read
        // on to, so it is never answered as not saved. Reading on stops at
        // the head the read has seen, or when a read makes no progress.
        let mut downloaded = None;
        loop {
            let committed = self
                .inner
                .storage
                .read_committed(session_id.clone())
                .await?;
            let Some(read) = committed.as_ref() else {
                return Ok(None);
            };
            if read.id() != &session_id {
                return Err(ConversationError::Storage(StorageError::IdentityMismatch));
            }
            let note = found(&committed);
            if note.is_some()
                || read.downloaded() >= read.observed_head()
                || downloaded.is_some_and(|before| read.downloaded() <= before)
            {
                return Ok(note);
            }
            downloaded = Some(read.downloaded());
        }
    }
}
