//! Existing SDK interest and UUID namespace adapters; no notification authority.

use crate::conversation::application::{WatchNamespaces, WatchRecords};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::{
    application::agent_execution::sessions::{ChangeWatchError, CommittedChangeWatch},
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::RecordStorage,
};
use std::sync::Arc;
use uuid::Uuid;

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

/// OS randomness is read only by this composed namespace adapter.
pub struct UuidWatchNamespaces;

impl WatchNamespaces for UuidWatchNamespaces {
    fn mint(&self) -> Uuid {
        Uuid::new_v4()
    }
}
