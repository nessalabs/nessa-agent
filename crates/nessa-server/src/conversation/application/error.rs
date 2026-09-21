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
