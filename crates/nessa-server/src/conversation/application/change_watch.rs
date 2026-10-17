//! Narrow injection of the existing committed-record producer.

use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::sessions::{ChangeWatchError, CommittedChangeWatch};

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

    /// Install interest in a commit of any conversation: for a reader of
    /// something drawn from many, such as a list's `running` flags.
    fn watch_any(&self) -> Result<CommittedChangeWatch, ChangeWatchError>;
}
