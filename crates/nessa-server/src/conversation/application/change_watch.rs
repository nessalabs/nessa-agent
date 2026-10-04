//! Narrow injection of the existing committed-record producer.

use crate::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::sessions::{ChangeWatchError, CommittedChangeWatch};
use uuid::Uuid;

/// Register payloadless record interest after current passive admission.
///
/// Registration performs no source read or writer admission. Its returned
/// non-cloneable handle retains the actual producer registration until drop.
pub trait WatchRecords: Send + Sync {
    /// Install interest in this stable conversation identity.
    fn watch(
        &self,
        conversation: &ConversationId,
    ) -> Result<CommittedChangeWatch, ChangeWatchError>;
}

/// Connection namespace minting, injected separately from authority and source IO.
pub trait WatchNamespaces: Send + Sync {
    /// Mint a new opaque UUID namespace for one physical product connection.
    fn mint(&self) -> Uuid;
}
