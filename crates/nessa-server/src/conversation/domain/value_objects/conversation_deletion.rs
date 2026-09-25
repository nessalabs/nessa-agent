use crate::conversation::domain::Conversation;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;

/// What a deleted conversation's saved history said about the provider's own
/// session, read once, before that history was erased.
///
/// The provider keeps its own transcript under this identity, and execution
/// evidence about permissions is keyed by it. Once the history is gone, this is
/// the only link from the conversation to either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderSessionLink {
    /// Not read yet: the conversation's agent has not been confirmed stopped,
    /// and until it is, what its history names can still change.
    Unread,
    /// The history named no provider session: the agent never attached.
    Absent,
    /// The provider session the history named.
    Recorded(ExecutionSessionId),
    /// The history was opened — its lease is there — and nothing of it can be
    /// read: an operator moved a damaged journal aside, or it was never
    /// written. Which provider session it named cannot be known, and is not
    /// taken to be none
    /// (`a_history_moved_aside_is_recorded_as_unknown_and_never_as_no_provider_session`).
    Unknown,
}

/// What became of the agent's own record of the provider session, once that
/// was settled. Only what the agent's binding says its answer means: the
/// agent's store is the agent's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderSessionErasure {
    /// The history named no provider session, so there was nothing to ask
    /// the agent to delete.
    NoProviderSession,
    /// Which provider session the history named could not be known — its
    /// journal was gone, with its lease still there — so no agent was asked.
    /// Whatever the agent kept of it is still there.
    SessionUnknown,
    /// The agent accepted the delete, and its adapter erases its record on it.
    Deleted,
    /// The agent accepted the delete, and its adapter archives its record
    /// rather than erasing it. The record is still in the agent's store.
    Archived,
    /// The agent accepted the delete, and what its adapter does on it is not
    /// known. Nothing is claimed about the record.
    Acknowledged,
    /// The agent refused the delete of a session its own list, read in full
    /// after the refusal, does not name: what it says of one it no longer
    /// has — typically because an earlier try deleted it and was interrupted
    /// before that was written down. Only that it was refused and not listed
    /// is claimed.
    NotListed,
    /// The agent does not offer deleting a session, so it was not asked. Its
    /// record is still there.
    NotSupported,
    /// The conversation names an agent this build has no adapter for, so no
    /// run of it can ask that agent. Its record is still there. An agent this
    /// build has an adapter for but did not start this run is not this: its
    /// deletion stays unfinished until it can be asked.
    NoHandler,
}

/// Why a tombstone cannot stand beside the conversation it is read with. A
/// tombstone that says one of these is refused, not repaired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeletionContradiction {
    /// Somebody other than the conversation's owner — another principal, or
    /// the same principal in another organization — is named as deleting it.
    InitiatorNotOwner,
    /// The deletion is dated before the conversation was created.
    BeforeCreation,
    /// Its progress is not a state a deletion can reach: an erasure of the
    /// provider session settled before the history was read, or one that does
    /// not agree with what the history named, or an erasure finished before
    /// the provider session was settled.
    ImpossibleProgress,
}

/// A caller's decision to delete a conversation: the tombstone its identity
/// keeps for good once the rest of it is erased.
///
/// It records who decided, through which surface, as which request, and when,
/// so that a repeated delete finishes the first one's work in the first one's
/// name rather than claiming a second deletion; and how far that work got —
/// the provider session its history named, what became of the agent's record
/// of it, and whether everything this server holds was erased — so that
/// nothing finished is done twice. Nothing here is a projection of what was
/// said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationDeletion {
    organization: OrganizationId,
    initiator: PrincipalId,
    surface: String,
    request: String,
    requested_at_ms: u64,
    provider_session: ProviderSessionLink,
    provider_erasure: Option<ProviderSessionErasure>,
    erased: bool,
}
impl ConversationDeletion {
    /// A deletion `initiator` of `organization` requested through `surface`
    /// as action `request`. Nothing of it is done yet.
    ///
    /// The surface and request are held to the same rule as a creator's, since
    /// both are written into durable evidence.
    pub fn new(
        organization: OrganizationId,
        initiator: PrincipalId,
        surface: String,
        request: String,
        requested_at_ms: u64,
    ) -> Result<Self, &'static str> {
        Conversation::check_creator_context(&surface, &request)?;
        Ok(Self {
            organization,
            initiator,
            surface,
            request,
            requested_at_ms,
            provider_session: ProviderSessionLink::Unread,
            provider_erasure: None,
            erased: false,
        })
    }
    /// A deletion read back from storage, with however far it had got.
    ///
    /// # Errors
    /// [`DeletionContradiction::ImpossibleProgress`] for progress no deletion
    /// reaches (`a_restored_tombstone_that_contradicts_itself_or_its_conversation_is_refused`).
    pub fn restore(
        decision: Self,
        provider_session: ProviderSessionLink,
        provider_erasure: Option<ProviderSessionErasure>,
        erased: bool,
    ) -> Result<Self, DeletionContradiction> {
        // Reading a history that named nothing, or could not be read,
        // settles it in the same step, so those never stand unsettled; a
        // session that was read is settled only by asking about it.
        let settled_as_expected = match (&provider_session, provider_erasure) {
            (ProviderSessionLink::Unread, erasure) => erasure.is_none(),
            (ProviderSessionLink::Absent, erasure) => {
                erasure == Some(ProviderSessionErasure::NoProviderSession)
            }
            (ProviderSessionLink::Unknown, erasure) => {
                erasure == Some(ProviderSessionErasure::SessionUnknown)
            }
            (ProviderSessionLink::Recorded(_), None) => true,
            (ProviderSessionLink::Recorded(_), Some(erasure)) => !matches!(
                erasure,
                ProviderSessionErasure::NoProviderSession | ProviderSessionErasure::SessionUnknown
            ),
        };
        if !settled_as_expected || (erased && provider_erasure.is_none()) {
            return Err(DeletionContradiction::ImpossibleProgress);
        }
        Ok(Self {
            provider_session,
            provider_erasure,
            erased,
            ..decision
        })
    }
    /// Whether `other` is this same decision: the same caller — organization,
    /// principal, and surface — asking as the same request. When it asked is
    /// not part of it, because a repeat of a request arrives later. The one
    /// rule for "the deciding request", which the service answers `applied`
    /// by and which [`Self::followed_by`] keeps progress by
    /// (`the_deciding_request_is_the_same_caller_on_the_same_surface_asking_again`).
    pub fn is_same_decision(&self, other: &Self) -> bool {
        self.organization == other.organization
            && self.initiator == other.initiator
            && self.surface == other.surface
            && self.request == other.request
    }
    /// This deletion, having read `session` from the conversation's history.
    /// A history that names none settles the provider session at once: there
    /// is nothing to ask the agent to delete.
    ///
    /// Read once. A deletion that has already read its history keeps what it
    /// read: by the time it is asked again the history may be erased, and an
    /// empty history then is not evidence that there never was a session
    /// (`a_tombstone_keeps_the_first_decision_and_reads_its_history_once`).
    pub fn after_reading(&self, session: Option<ExecutionSessionId>) -> Self {
        if self.provider_session != ProviderSessionLink::Unread {
            return self.clone();
        }
        let (provider_session, provider_erasure) = match session {
            Some(session) => (ProviderSessionLink::Recorded(session), None),
            None => (
                ProviderSessionLink::Absent,
                Some(ProviderSessionErasure::NoProviderSession),
            ),
        };
        Self {
            provider_session,
            provider_erasure,
            ..self.clone()
        }
    }
    /// This deletion, having found its history's lease with nothing of the
    /// history to read. Settled at once as
    /// [`ProviderSessionErasure::SessionUnknown`]: no agent can be asked about
    /// a session nobody knows. Read once, like [`Self::after_reading`].
    pub fn after_losing_history(&self) -> Self {
        if self.provider_session != ProviderSessionLink::Unread {
            return self.clone();
        }
        Self {
            provider_session: ProviderSessionLink::Unknown,
            provider_erasure: Some(ProviderSessionErasure::SessionUnknown),
            ..self.clone()
        }
    }
    /// This deletion, once what became of the agent's record of the provider
    /// session it read is settled as `erasure`.
    ///
    /// Settled once, and only for a provider session it read: before the
    /// history is read there is no session to settle, and
    /// [`ProviderSessionErasure::NoProviderSession`] is settled by reading a
    /// history that names none. Anything else leaves this unchanged.
    pub fn after_provider_erasure(&self, erasure: ProviderSessionErasure) -> Self {
        let recorded = matches!(self.provider_session, ProviderSessionLink::Recorded(_));
        if !recorded
            || self.provider_erasure.is_some()
            || matches!(
                erasure,
                ProviderSessionErasure::NoProviderSession | ProviderSessionErasure::SessionUnknown
            )
        {
            return self.clone();
        }
        Self {
            provider_erasure: Some(erasure),
            ..self.clone()
        }
    }
    /// This deletion, once everything the server held of the conversation is
    /// erased; `None` while what became of the agent's record of the provider
    /// session is not settled, since a deletion cannot be finished before
    /// that (`a_tombstone_keeps_the_first_decision_and_reads_its_history_once`).
    pub fn after_erasure(&self) -> Option<Self> {
        self.provider_erasure.is_some().then(|| Self {
            erased: true,
            ..self.clone()
        })
    }
    /// The tombstone this one becomes when `later` is recorded over it.
    ///
    /// The first decision stands: a later delete by another request, or
    /// another caller, changes nothing about who deleted the conversation or
    /// when. The same decision may carry it further, and only further: what
    /// was read, settled, or erased stays so
    /// (`a_tombstone_keeps_the_first_decision_and_reads_its_history_once`).
    pub fn followed_by(&self, later: &Self) -> Self {
        if !self.is_same_decision(later) {
            return self.clone();
        }
        let read = match &later.provider_session {
            ProviderSessionLink::Unread => self.clone(),
            ProviderSessionLink::Absent => self.after_reading(None),
            ProviderSessionLink::Recorded(session) => self.after_reading(Some(session.clone())),
            ProviderSessionLink::Unknown => self.after_losing_history(),
        };
        let settled = match later.provider_erasure {
            Some(erasure) => read.after_provider_erasure(erasure),
            None => read,
        };
        match later.erased.then(|| settled.after_erasure()).flatten() {
            Some(finished) => finished,
            None => settled,
        }
    }
    /// The organization of the caller who deleted it.
    pub fn organization(&self) -> &OrganizationId {
        &self.organization
    }
    pub fn initiator(&self) -> &PrincipalId {
        &self.initiator
    }
    pub fn surface(&self) -> &str {
        &self.surface
    }
    /// The action identifier of the request that decided the deletion.
    pub fn request(&self) -> &str {
        &self.request
    }
    pub fn requested_at_ms(&self) -> u64 {
        self.requested_at_ms
    }
    pub fn provider_session(&self) -> &ProviderSessionLink {
        &self.provider_session
    }
    /// What became of the agent's record of the provider session, once
    /// settled; `None` until then.
    pub fn provider_erasure(&self) -> Option<ProviderSessionErasure> {
        self.provider_erasure
    }
    /// Whether everything this server held of the conversation was erased,
    /// so the deletion is finished and nothing of it is attempted again.
    pub fn erased(&self) -> bool {
        self.erased
    }
}
