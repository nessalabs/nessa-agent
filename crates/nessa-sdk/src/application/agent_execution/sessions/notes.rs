//! Notes in a conversation's stream: what `conversation.message` leaves, a
//! text with its author, saved without starting or steering a turn (ADR 0011
//! phase B; slice J3 of #707).
//!
//! ```text
//! NoteRequest ──admit(snapshot, now)──▶ NoteRecord appended to SessionSnapshot::notes
//!      │                  │
//!      │                  └─▶ NoteRefusal::{Full, RateLimited, Conflict}
//!      └─▶ the same request already recorded ──▶ that record, unchanged
//! ```
//!
//! One rule decides a new note and reads a saved one back
//! ([`validate_saved`]): at most [`NoteRecord::MAX_PER_CONVERSATION`] notes,
//! each with its own id, and one note per principal and request. The rate
//! bound ([`NoteRecord::MAX_PER_AUTHOR_IN_WINDOW`]) is asked only of a new
//! note, at the time it is accepted: a saved note was accepted then, and a
//! later clock cannot make it wrong.
//!
//! A `record_only` note is never handed to the model. Delivering a note to
//! the next turn (`next_turn`) is slice J3b.
use super::{SessionSnapshot, StorageError};
use crate::application::agent_execution::permissions::ActionContext;
use crate::domain::agent_execution::executions::Initiator;
use std::{collections::HashSet, error::Error, fmt};
use uuid::Uuid;

/// A value of a note is outside what one may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteValueError {
    /// The id is blank or longer than [`NoteId::MAX_BYTES`].
    Id,
    /// The text is blank or longer than [`NoteText::MAX_BYTES`].
    Text,
}
impl fmt::Display for NoteValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id => write!(f, "a note id must be 1 to {} bytes", NoteId::MAX_BYTES),
            Self::Text => write!(
                f,
                "a note must hold 1 to {} bytes of text that is not all blank",
                NoteText::MAX_BYTES
            ),
        }
    }
}
impl Error for NoteValueError {}

/// The conversation's own name for one note, made when it is accepted and
/// never chosen by its author.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NoteId(Box<str>);
impl NoteId {
    /// Longest id, in bytes. A minted id is a 36-byte UUID.
    pub const MAX_BYTES: usize = 64;

    /// A fresh id: a UUID v4, which no other note can share.
    pub fn mint() -> Self {
        Self(Uuid::new_v4().to_string().into_boxed_str())
    }
    /// An id read back: 1 to [`Self::MAX_BYTES`] bytes, not blank; otherwise
    /// [`NoteValueError::Id`].
    pub fn new(value: impl Into<String>) -> Result<Self, NoteValueError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > Self::MAX_BYTES {
            return Err(NoteValueError::Id);
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a note says: text, kept exactly as given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteText(Box<str>);
impl NoteText {
    /// Longest text, in bytes: the bound a sent message's text has.
    pub const MAX_BYTES: usize = 8192;

    /// Accept 1 to [`Self::MAX_BYTES`] bytes of text that is not all blank;
    /// otherwise [`NoteValueError::Text`]. A blank note says nothing.
    pub fn new(value: impl Into<String>) -> Result<Self, NoteValueError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > Self::MAX_BYTES {
            return Err(NoteValueError::Text);
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// The text as given.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What becomes of a note once it is saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteDelivery {
    /// Saved with its author, and never handed to the model by itself.
    RecordOnly,
}

/// Where a saved note stands. A `record_only` note is recorded, and stays
/// recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteState {
    /// Saved, with nothing waiting to deliver it.
    Recorded,
}

/// A note as its host admitted it, before the conversation accepts it: the
/// author and actor are the host's verified answer, never the caller's say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteRequest {
    /// What it says.
    pub text: NoteText,
    /// What becomes of it.
    pub delivery: NoteDelivery,
    /// Who sent it, from which surface, under which request. The principal
    /// and request together name the command: a retry carries both.
    pub actor: ActionContext,
    /// The authority it was admitted under: the conversation's owner, or the
    /// share grant at the revision the host checked. A later slice checks a
    /// queued note's grant again by this before delivering it.
    pub author: Initiator,
}

/// One note the conversation accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteRecord {
    /// Its id, made when it was accepted.
    pub id: NoteId,
    /// What it says.
    pub text: NoteText,
    /// What becomes of it.
    pub delivery: NoteDelivery,
    /// Who sent it, as [`NoteRequest::actor`].
    pub actor: ActionContext,
    /// Under which authority, as [`NoteRequest::author`].
    pub author: Initiator,
    /// When it was accepted, in milliseconds since the Unix epoch, on the
    /// conversation's injected wall clock.
    pub accepted_at_ms: u64,
}
impl NoteRecord {
    /// Most notes one conversation keeps. Another is refused rather than an
    /// earlier one dropped.
    pub const MAX_PER_CONVERSATION: usize = 1024;
    /// Most notes one author, a principal under one authority, leaves in any
    /// [`Self::RATE_WINDOW_MS`].
    pub const MAX_PER_AUTHOR_IN_WINDOW: usize = 20;
    /// The window [`Self::MAX_PER_AUTHOR_IN_WINDOW`] counts over.
    pub const RATE_WINDOW_MS: u64 = 60_000;

    /// Where the note stands.
    pub fn state(&self) -> NoteState {
        match self.delivery {
            NoteDelivery::RecordOnly => NoteState::Recorded,
        }
    }

    /// Whether this note answers `principal`'s request `request`: one note
    /// per principal and request in a conversation.
    pub fn answers(&self, principal: &str, request: &str) -> bool {
        self.actor.principal_id() == principal && self.actor.request_id() == request
    }

    /// Whether `request` is this note sent again: the same text and
    /// delivery, from the same principal and surface, under the same request
    /// and authority. A retry admitted after its grant's policy changed is
    /// still this note, and keeps the revision it was first accepted under,
    /// as a turn's retry does.
    fn is_same_request(&self, request: &NoteRequest) -> bool {
        self.text == request.text
            && self.delivery == request.delivery
            && self.actor == request.actor
            && same_authority(&self.author, &request.author)
    }

    /// Whether `request` comes from this note's author: the same principal
    /// under the same authority, whatever its grant's revision, so a change
    /// of policy gives no fresh rate.
    fn same_author(&self, request: &NoteRequest) -> bool {
        self.actor.principal_id() == request.actor.principal_id()
            && same_authority(&self.author, &request.author)
    }
}

/// Whether two authors are the same authority: the owner, or the same grant
/// at any revision.
fn same_authority(left: &Initiator, right: &Initiator) -> bool {
    match (left, right) {
        (Initiator::Owner, Initiator::Owner) => true,
        (Initiator::Grant(left), Initiator::Grant(right)) => left.grant() == right.grant(),
        _ => false,
    }
}

/// How the conversation answers a note request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NoteAdmission {
    /// A note it does not hold yet: accept it as this record.
    New(NoteRecord),
    /// The same request already recorded: that record answers it.
    Recorded(NoteRecord),
}

/// Why a note request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NoteRefusal {
    /// The principal's request already left another note.
    Conflict,
    /// The conversation keeps its most notes.
    Full,
    /// The author left its most notes in the current window.
    RateLimited,
}

/// How [`NoteRequest`] is answered by what `snapshot` already records, at
/// wall-clock time `now_ms`. The one rule a new note follows: the same
/// request is answered by its record, another under the same request is a
/// conflict, and a new one must fit the conversation's bound and its
/// author's rate.
pub(super) fn admit(
    snapshot: &SessionSnapshot,
    request: NoteRequest,
    now_ms: u64,
) -> Result<NoteAdmission, NoteRefusal> {
    if let Some(recorded) = snapshot
        .notes
        .iter()
        .find(|note| note.answers(request.actor.principal_id(), request.actor.request_id()))
    {
        return if recorded.is_same_request(&request) {
            Ok(NoteAdmission::Recorded(recorded.clone()))
        } else {
            Err(NoteRefusal::Conflict)
        };
    }
    if snapshot.notes.len() >= NoteRecord::MAX_PER_CONVERSATION {
        return Err(NoteRefusal::Full);
    }
    // A note accepted later than now, by a clock since set back, counts too:
    // setting the clock back never makes room.
    let recent = snapshot
        .notes
        .iter()
        .filter(|note| {
            note.same_author(&request)
                && note
                    .accepted_at_ms
                    .saturating_add(NoteRecord::RATE_WINDOW_MS)
                    > now_ms
        })
        .count();
    if recent >= NoteRecord::MAX_PER_AUTHOR_IN_WINDOW {
        return Err(NoteRefusal::RateLimited);
    }
    Ok(NoteAdmission::New(NoteRecord {
        id: NoteId::mint(),
        text: request.text,
        delivery: request.delivery,
        actor: request.actor,
        author: request.author,
        accepted_at_ms: now_ms,
    }))
}

/// Whether `record` may follow what `snapshot` already records, read back
/// from a record log: within the bound, with an id and a principal's
/// request no earlier note has.
pub(super) fn admit_saved(
    snapshot: &SessionSnapshot,
    record: &NoteRecord,
) -> Result<(), StorageError> {
    if snapshot.notes.len() >= NoteRecord::MAX_PER_CONVERSATION {
        return Err(corrupt("the conversation keeps more notes than it may"));
    }
    let repeated = snapshot.notes.iter().any(|note| {
        note.id == record.id || note.answers(record.actor.principal_id(), record.actor.request_id())
    });
    if repeated {
        return Err(corrupt("two notes share an id or a request"));
    }
    Ok(())
}

/// Whether the notes a saved snapshot holds keep the rule [`admit_saved`]
/// asks of each.
pub(super) fn validate_saved(snapshot: &SessionSnapshot) -> Result<(), StorageError> {
    if snapshot.notes.len() > NoteRecord::MAX_PER_CONVERSATION {
        return Err(corrupt("the conversation keeps more notes than it may"));
    }
    let mut ids = HashSet::new();
    let mut requests = HashSet::new();
    for note in &snapshot.notes {
        if !ids.insert(note.id.as_str())
            || !requests.insert((note.actor.principal_id(), note.actor.request_id()))
        {
            return Err(corrupt("two notes share an id or a request"));
        }
    }
    Ok(())
}

fn corrupt(reason: &str) -> StorageError {
    StorageError::Corrupt(format!("note record: {reason}"))
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/notes.rs"]
mod tests;
