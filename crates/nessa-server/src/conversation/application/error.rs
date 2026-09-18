use nessa_sdk::application::agent_execution::{
    agents::AgentError, permissions::PermissionSelectionState, sessions::StorageError,
};
use std::{error::Error, fmt};

/// Service errors retain internal SDK evidence. Wire adapters expose safe error codes.
#[derive(Clone, Debug)]
pub enum ConversationError {
    InvalidInput,
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
