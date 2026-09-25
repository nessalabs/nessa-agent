use crate::conversation::domain::ConversationRefusal;
use nessa_sdk::application::agent_execution::{
    agents::AgentError, permissions::PermissionSelectionState, sessions::StorageError,
};
use std::{error::Error, fmt};

/// Service errors retain internal SDK evidence. Wire adapters expose safe error codes.
#[derive(Clone, Debug)]
pub enum ConversationError {
    InvalidInput,
    /// The connected agent, or this gateway's configuration, takes no images.
    ImagesUnsupported,
    /// The message refers to an image this conversation has not uploaded.
    AttachmentNotFound,
    /// The conversation closed, but letting go of its uploads did not complete.
    AttachmentRelease(Box<ConversationError>),
    /// What a release of uploads could not complete, after all of it was
    /// tried. The two are different failures and are kept apart: files still
    /// in place, and transitions that happened without acknowledged evidence.
    AttachmentCleanup {
        storage_failures: usize,
        audit_failures: usize,
    },
    /// Closing failed twice over: the agent did not close (or could not be
    /// reached to close), and letting go of the uploads did not complete
    /// either. Neither replaces the other.
    CloseIncomplete {
        agent: Box<ConversationError>,
        release: Box<ConversationError>,
    },
    NotFound,
    /// The caller's conversation was deleted. Every command on it is refused
    /// for good, a creation of its identity included
    /// (`a_deleted_conversation_refuses_every_command_on_it`).
    Deleted,
    /// A delete that happened — the conversation is tombstoned, left out of
    /// every list, and refused — and did not finish. Repeating the delete
    /// finishes it.
    DeletionIncomplete(Box<DeletionFailures>),
    /// The agent asked for is one this server has no configuration to start.
    /// Kept apart from an invalid request because the request was valid and the
    /// answer is about this installation, which is something setup can fix.
    AgentNotConfigured,
    /// The conversation on disk names an agent this build has no adapter for.
    ///
    /// Apart from [`Self::Metadata`] because the record was read perfectly well
    /// and storage is working: what cannot be done is open it, and never on
    /// this build. Telling the caller storage was unavailable would invite a
    /// retry that can only fail the same way.
    AgentUnsupported,
    Capacity,
    Unavailable,
    Metadata,
    Audit,
    /// The SDK owns the submission receipt, but one or both mandatory evidence
    /// stores did not acknowledge admission.
    AdmissionEvidence {
        audit: Option<AgentError>,
        storage: Option<StorageError>,
    },
    /// Every failed owner is retained; successful cleanup of another owner never erases it.
    Retirement(Vec<(String, AgentError)>),
    /// Retirement attempted cleanup despite an unsettled admitted command.
    RetirementAdmission {
        cleanup_error: Option<Box<ConversationError>>,
    },
    Agent(AgentError),
    /// A permission-answer failure retains whether the domain review was consumed.
    PermissionAnswer {
        error: AgentError,
        selection: PermissionSelectionState,
    },
    Storage(StorageError),
}
impl fmt::Display for ConversationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "conversation service: {self:?}")
    }
}
impl Error for ConversationError {}
impl From<ConversationRefusal> for ConversationError {
    fn from(refusal: ConversationRefusal) -> Self {
        match refusal {
            ConversationRefusal::NotFound => Self::NotFound,
            ConversationRefusal::Deleted => Self::Deleted,
        }
    }
}
impl From<AgentError> for ConversationError {
    fn from(error: AgentError) -> Self {
        Self::Agent(error)
    }
}
impl From<StorageError> for ConversationError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

/// Why a conversation's agent could not be confirmed stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopFailure {
    /// The stop budget ran out while the stop was still under way: it may yet
    /// be confirmed, so a deletion waiting on it is carried on.
    OverBudget,
    /// The stop itself failed, whatever the error — a close that answered
    /// [`AgentError::Deadline`] of its own included.
    Failed(AgentError),
}

/// What a delete that happened could not finish, each part kept apart.
///
/// The order is the delete's own. An agent not confirmed stopped means nothing
/// after it was tried. A history that could not be read, an agent whose own
/// record of the session could not be asked about, or a deletion record the
/// sink did not take, means the history and summary were kept; uploads were
/// still let go, since the stopped agent that could have used them is gone.
/// Otherwise each store was tried and each field says what remains in it.
#[derive(Clone, Debug, Default)]
pub struct DeletionFailures {
    /// The conversation's agent could not be confirmed stopped.
    pub stop: Option<StopFailure>,
    /// The saved history could not be read or erased: storage failed. What
    /// was read and could not be kept in the tombstone is
    /// [`Self::tombstone`]'s; a lease held elsewhere is
    /// [`Self::history_held`].
    pub history: Option<ConversationError>,
    /// The history's lease was still held elsewhere once the delete's wait
    /// for it ran out, so the history was neither read nor erased. Only that
    /// wait sets this — storage answering `Busy` under the deletion's own
    /// lease does not — and it is what a deletion is carried on for until the
    /// lease is let go
    /// (`busy_under_the_deletion_s_own_lease_is_left_not_carried_on`).
    pub history_held: bool,
    /// The agent could not be asked to delete its own record of the provider
    /// session, or refused: `ConversationError::Agent` with the typed cause.
    pub provider: Option<ConversationError>,
    /// Every agent slot on this gateway was taken, so the agent was not asked.
    /// Only the gateway's own slot bound sets this — no agent's answer can —
    /// and it is what a deletion is carried on for until a slot frees.
    pub no_agent_slot: bool,
    /// The deletion record was not acknowledged.
    pub audit: Option<ConversationError>,
    /// Letting go of uploads did not complete.
    pub attachments: Option<ConversationError>,
    /// The summary could not be erased.
    pub summary: Option<ConversationError>,
    /// How far the deletion got could not be written to its tombstone, or
    /// the tombstone could not be read back, so the next attempt repeats
    /// what this one did.
    pub tombstone: Option<ConversationError>,
    /// A step after the fence panicked: the attempt ended there, and what it
    /// left is what the tombstone says.
    pub interrupted: bool,
    /// Another attempt was carrying this deletion when the request arrived,
    /// and it did not finish. The request waited for it and answered from
    /// the tombstone it left, without a second attempt: what is left is
    /// whatever that attempt left.
    pub another_attempt: bool,
}
impl DeletionFailures {
    /// Whether every part finished.
    pub fn is_empty(&self) -> bool {
        self.stop.is_none()
            && self.history.is_none()
            && !self.history_held
            && self.provider.is_none()
            && !self.no_agent_slot
            && self.audit.is_none()
            && self.attachments.is_none()
            && self.summary.is_none()
            && self.tombstone.is_none()
            && !self.interrupted
            && !self.another_attempt
    }
}
