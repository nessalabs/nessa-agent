use super::super::ConversationCaller;
use crate::conversation::domain::{BackgroundCommand, BackgroundTaskId};
use nessa_protocol::conversation::domain::ConversationId;

/// Source facts resolved once at admission, not overwritten by a retry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundSource {
    pub conversation: ConversationId,
    pub execution: String,
    pub tool_call: String,
    pub caller: ConversationCaller,
    pub workspace: String,
}

/// Source and command transport around the domain-owned lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundRecord {
    pub id: BackgroundTaskId,
    pub source: BackgroundSource,
    pub command: BackgroundCommand,
    pub lifecycle: crate::conversation::domain::TaskLifecycle,
}

impl BackgroundRecord {
    /// Fixed across retries and gateway restarts; one internal turn per task.
    pub fn continuation_id(&self) -> String {
        format!("background-{}", self.id.as_str())
    }
}
