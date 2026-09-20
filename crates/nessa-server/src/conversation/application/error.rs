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
    NotFound,
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
