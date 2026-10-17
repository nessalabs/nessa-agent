//! The SDK's committed-record interest, adapted; no notification authority.

use crate::conversation::application::WatchRecords;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::{
    application::agent_execution::sessions::{ChangeWatchError, CommittedChangeWatch},
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::RecordStorage,
};
use std::sync::Arc;

/// Retains the same physical RecordStorage composed for reads and writers.
pub struct NessaRecordWatches {
    storage: Arc<RecordStorage>,
}

impl NessaRecordWatches {
    /// Adapt an already composed storage owner; never reopen storage.
    pub fn new(storage: Arc<RecordStorage>) -> Self {
        Self { storage }
    }
}

impl WatchRecords for NessaRecordWatches {
    fn watch(
        &self,
        conversation: &ConversationId,
    ) -> Result<CommittedChangeWatch, ChangeWatchError> {
        let id = SessionId::new(conversation.to_string())
            .expect("validated conversation UUID is a valid SDK session identity");
        self.storage.watch_committed(&id)
    }

    fn watch_any(&self) -> Result<CommittedChangeWatch, ChangeWatchError> {
        self.storage.watch_any_committed()
    }
}
