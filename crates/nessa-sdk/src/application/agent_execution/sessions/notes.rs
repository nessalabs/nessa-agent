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
//! A `record_only` note is never handed to the model. A `next_turn` note
//! (slice J3b) waits, pending, for the next turn a person starts while the
//! agent is idle ([`select`]):
//!
//! ```text
//! pending ──turn admitted, author still allowed──▶ assigned(turn)
//!    │
//!    ├──lifetime over, at a turn's admission──▶ retired(expired)
//!    └──host says the grant is gone──────────▶ retired(permission_lost)
//! ```
//!
//! Each fact has one record: the note itself ([`SessionChange::Note`]), the
//! turn that took it (its [`InvocationRecord::notes`]), and a retirement
//! ([`SessionChange::NoteRetired`]). Where a note stands is read from those
//! ([`SessionSnapshot::note_state`]) and never stored twice. An assigned
//! note is never pending again, whatever becomes of its turn.
//!
//! [`SessionChange::Note`]: super::SessionChange::Note
//! [`SessionChange::NoteRetired`]: super::SessionChange::NoteRetired
//! [`InvocationRecord::notes`]: super::InvocationRecord::notes
use super::{SessionSnapshot, StorageError};
use crate::application::agent_execution::permissions::ActionContext;
use crate::domain::agent_execution::executions::{ExecutionId, Initiator};
use crate::domain::agent_execution::prompts::{NoteAuthor, NoteContext, UserMessage};
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
};
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
    /// Saved, and handed to the model with the next turn a person starts
    /// while the agent is idle, if its author may still leave it then.
    NextTurn,
}

/// Where a saved note stands, read from the conversation's records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteState {
    /// A `record_only` note: saved, with nothing waiting to deliver it.
    Recorded,
    /// A `next_turn` note waiting for a turn to take it.
    Pending,
    /// Taken by this turn, whose saved acceptance names it. It is never
    /// pending again, whether or not the turn ran to its end.
    Assigned(ExecutionId),
    /// Retired before any turn took it.
    Retired(NoteRetirement),
}

/// Why a pending note was retired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteRetirementReason {
    /// It waited past [`NoteRecord::PENDING_LIFETIME_MS`].
    Expired,
    /// Its author's grant was revoked, or lowered below Comment, before a
    /// turn took it.
    PermissionLost,
}

/// A pending note's retirement: why, and when, on the conversation's wall
/// clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteRetirement {
    /// Why it was retired.
    pub reason: NoteRetirementReason,
    /// When, in milliseconds since the Unix epoch.
    pub at_ms: u64,
}

/// One retirement as the records keep it: which note, and the retirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteRetiredRecord {
    /// The note retired.
    pub id: NoteId,
    /// Why and when.
    pub retirement: NoteRetirement,
}

/// The host's answer, for one turn, on which pending notes' authors may
/// still leave them (ADR 0011 phase B): the conversation decides which of
/// these a turn takes, and the host decides nothing more.
///
/// A note in neither list could not be checked now: it stays pending, and no
/// turn takes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteCandidates {
    /// Notes whose author may still leave them.
    pub authorized: Vec<NoteId>,
    /// Notes whose author's grant is gone, or below Comment: retired.
    pub revoked: Vec<NoteId>,
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
    /// The last turn the conversation had accepted when this note was, so a
    /// transcript places the note after it; `None` before any turn.
    pub after_execution: Option<ExecutionId>,
    /// Its retirement, folded from the conversation's
    /// [`SessionChange::NoteRetired`](super::SessionChange::NoteRetired);
    /// `None` when none was saved. A note is accepted without one.
    pub retired: Option<NoteRetirement>,
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
    /// Most `next_turn` notes waiting at once in one conversation.
    pub const MAX_PENDING: usize = 64;
    /// How long a `next_turn` note waits for a turn before it is retired.
    pub const PENDING_LIFETIME_MS: u64 = 24 * 60 * 60 * 1000;
    /// Most notes one turn takes: what a message may carry.
    pub const MAX_PER_TURN: usize = UserMessage::MAX_NOTES;
    /// Latest accepted time a note may carry: the largest integer a JSON
    /// number holds exactly, so a receipt reads back unchanged on every
    /// client.
    pub const MAX_ACCEPTED_AT_MS: u64 = (1 << 53) - 1;

    /// The note as the model is given it with the turn that takes it.
    ///
    /// # Errors
    ///
    /// [`StorageError::Corrupt`] for a note the prompt cannot carry, which a
    /// saved note never is: its text and surface are within the same bounds.
    pub fn context(&self) -> Result<NoteContext, StorageError> {
        let author = match self.author {
            Initiator::Owner => NoteAuthor::Owner,
            Initiator::Grant(_) => NoteAuthor::Grant,
            Initiator::Agent(_) => NoteAuthor::Agent,
        };
        NoteContext::new(author, self.actor.surface_id(), self.text.as_str())
            .map_err(|error| corrupt(&error.to_string()))
    }

    /// Whether its lifetime is over at `now_ms`.
    fn overdue(&self, now_ms: u64) -> bool {
        self.accepted_at_ms
            .saturating_add(Self::PENDING_LIFETIME_MS)
            <= now_ms
    }
    /// Whether `retirement` is one this note can have: expired only once
    /// past its lifetime, and permission lost only for a grant's note, since
    /// the owner never loses the right to leave one.
    fn may_be_retired(&self, retirement: &NoteRetirement) -> bool {
        match retirement.reason {
            NoteRetirementReason::Expired => self.overdue(retirement.at_ms),
            NoteRetirementReason::PermissionLost => matches!(self.author, Initiator::Grant(_)),
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
        (Initiator::Agent(left), Initiator::Agent(right)) => left == right,
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
    // A note past its lifetime is retired by the next turn's admission, and
    // waits for nothing: it takes no room.
    if request.delivery == NoteDelivery::NextTurn
        && snapshot
            .notes
            .iter()
            .filter(|note| snapshot.note_state(note) == NoteState::Pending && !note.overdue(now_ms))
            .count()
            >= NoteRecord::MAX_PENDING
    {
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
    // A clock past the latest time a receipt can state holds there.
    let now_ms = now_ms.min(NoteRecord::MAX_ACCEPTED_AT_MS);
    Ok(NoteAdmission::New(NoteRecord {
        id: NoteId::mint(),
        text: request.text,
        delivery: request.delivery,
        actor: request.actor,
        author: request.author,
        accepted_at_ms: now_ms,
        after_execution: snapshot
            .invocations
            .last()
            .map(|record| record.request.execution_id.clone()),
        retired: None,
    }))
}

/// What a turn's admission does to the pending notes, at `now_ms`, given the
/// host's `candidates`: in saved order, a note past its lifetime is retired
/// `Expired`; one the host says lost its grant, `PermissionLost`; and, when
/// `takes_notes`, up to [`NoteRecord::MAX_PER_TURN`] the host authorized are
/// taken. Nothing else is touched: a note the host could not check waits.
pub(super) fn select(
    snapshot: &SessionSnapshot,
    candidates: &NoteCandidates,
    takes_notes: bool,
    now_ms: u64,
) -> NoteSelection {
    let mut selection = NoteSelection::default();
    for note in &snapshot.notes {
        if snapshot.note_state(note) != NoteState::Pending {
            continue;
        }
        let reason = if note.overdue(now_ms) {
            Some(NoteRetirementReason::Expired)
        } else if candidates.revoked.contains(&note.id)
            && matches!(note.author, Initiator::Grant(_))
        {
            Some(NoteRetirementReason::PermissionLost)
        } else {
            None
        };
        if let Some(reason) = reason {
            selection.retired.push(NoteRetiredRecord {
                id: note.id.clone(),
                retirement: NoteRetirement {
                    reason,
                    at_ms: now_ms,
                },
            });
        } else if takes_notes
            && selection.taken.len() < NoteRecord::MAX_PER_TURN
            && candidates.authorized.contains(&note.id)
        {
            selection.taken.push(note.id.clone());
        }
    }
    selection
}

/// What [`select`] decided for one turn's admission.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct NoteSelection {
    /// Pending notes to retire, in saved order.
    pub retired: Vec<NoteRetiredRecord>,
    /// Pending notes the turn takes, in saved order.
    pub taken: Vec<NoteId>,
}

/// Whether `record` may be folded into `snapshot`: it names a `next_turn`
/// note that is pending there. Answers that note's index.
pub(super) fn admit_retired(
    snapshot: &SessionSnapshot,
    record: &NoteRetiredRecord,
) -> Result<usize, StorageError> {
    let index = snapshot
        .notes
        .iter()
        .position(|note| note.id == record.id)
        .ok_or_else(|| corrupt("a retirement names no saved note"))?;
    if snapshot.note_state(&snapshot.notes[index]) != NoteState::Pending {
        return Err(corrupt("a retirement names a note that is not pending"));
    }
    if !snapshot.notes[index].may_be_retired(&record.retirement) {
        return Err(corrupt("a retirement gives a reason the note cannot have"));
    }
    Ok(index)
}

/// Whether the notes a turn names may be folded into `snapshot` with it: at
/// most [`NoteRecord::MAX_PER_TURN`], each a distinct `next_turn` note
/// pending there.
pub(super) fn admit_taken(
    snapshot: &SessionSnapshot,
    taken: &[NoteId],
) -> Result<(), StorageError> {
    if taken.len() > NoteRecord::MAX_PER_TURN {
        return Err(corrupt("a turn takes more notes than it may"));
    }
    for (index, id) in taken.iter().enumerate() {
        if taken[..index].contains(id) {
            return Err(corrupt("a turn names one note twice"));
        }
        let pending = snapshot
            .notes
            .iter()
            .find(|note| &note.id == id)
            .is_some_and(|note| snapshot.note_state(note) == NoteState::Pending);
        if !pending {
            return Err(corrupt("a turn takes a note that is not pending"));
        }
    }
    Ok(())
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
    if record.accepted_at_ms > NoteRecord::MAX_ACCEPTED_AT_MS {
        return Err(corrupt(
            "a note was accepted later than a receipt can state",
        ));
    }
    if record.retired.is_some() {
        return Err(corrupt("a note is accepted with no retirement"));
    }
    if !names_saved_turn(snapshot, record.after_execution.as_ref()) {
        return Err(corrupt(
            "a note follows a turn the conversation does not hold",
        ));
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
        if note.accepted_at_ms > NoteRecord::MAX_ACCEPTED_AT_MS {
            return Err(corrupt(
                "a note was accepted later than a receipt can state",
            ));
        }
        if !names_saved_turn(snapshot, note.after_execution.as_ref()) {
            return Err(corrupt(
                "a note follows a turn the conversation does not hold",
            ));
        }
        if note.retired.is_some() && note.delivery != NoteDelivery::NextTurn {
            return Err(corrupt("a record_only note is retired"));
        }
        if note
            .retired
            .as_ref()
            .is_some_and(|retirement| !note.may_be_retired(retirement))
        {
            return Err(corrupt("a retirement gives a reason the note cannot have"));
        }
        if !ids.insert(note.id.as_str())
            || !requests.insert((note.actor.principal_id(), note.actor.request_id()))
        {
            return Err(corrupt("two notes share an id or a request"));
        }
    }
    // Every note a turn took is a `next_turn` note no other turn took and
    // none retired, left before that turn: after the turn it follows.
    let position: HashMap<&ExecutionId, usize> = snapshot
        .invocations
        .iter()
        .enumerate()
        .map(|(index, record)| (&record.request.execution_id, index))
        .collect();
    let mut taken = HashSet::new();
    for (index, record) in snapshot.invocations.iter().enumerate() {
        if record.notes.len() > NoteRecord::MAX_PER_TURN {
            return Err(corrupt("a turn takes more notes than it may"));
        }
        for id in &record.notes {
            let note = snapshot
                .notes
                .iter()
                .find(|note| &note.id == id)
                .ok_or_else(|| corrupt("a turn takes a note the conversation does not hold"))?;
            let left_before = note
                .after_execution
                .as_ref()
                .is_none_or(|turn| position.get(turn).is_some_and(|after| *after < index));
            if note.delivery != NoteDelivery::NextTurn
                || note.retired.is_some()
                || !left_before
                || !taken.insert(id)
            {
                return Err(corrupt("a turn takes a note that was not pending"));
            }
        }
    }
    Ok(())
}

fn names_saved_turn(snapshot: &SessionSnapshot, turn: Option<&ExecutionId>) -> bool {
    turn.is_none_or(|turn| {
        snapshot
            .invocations
            .iter()
            .any(|record| &record.request.execution_id == turn)
    })
}

fn corrupt(reason: &str) -> StorageError {
    StorageError::Corrupt(format!("note record: {reason}"))
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/notes.rs"]
mod tests;
