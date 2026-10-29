//! Correlate indexed facts with the domain record and immutable admission audit.
use super::{encoding, BackgroundRecord, BackgroundTaskError};
use crate::conversation::domain::TaskLifecycle;
use nessa_local_database::rusqlite::{self, Row};

pub(super) const SELECT: &str = "SELECT t.conversation, t.task, t.execution, t.tool_call, t.revision, t.retained, t.record, a.evidence FROM tasks t LEFT JOIN transitions a ON a.conversation = t.conversation AND a.task = t.task AND a.revision = 0";

pub(super) struct StoredRow {
    conversation: String,
    task: String,
    execution: String,
    tool_call: String,
    revision: i64,
    retained: bool,
    record: String,
    admission: Option<String>,
}
impl StoredRow {
    pub(super) fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            conversation: row.get(0)?,
            task: row.get(1)?,
            execution: row.get(2)?,
            tool_call: row.get(3)?,
            revision: row.get(4)?,
            retained: row.get(5)?,
            record: row.get(6)?,
            admission: row.get(7)?,
        })
    }
    pub(super) fn decode(self) -> Result<BackgroundRecord, BackgroundTaskError> {
        let record = encoding::decode(&self.record)?;
        if record.source.conversation.to_string() != self.conversation
            || record.id.as_str() != self.task
            || record.source.execution != self.execution
            || record.source.tool_call != self.tool_call
            || i64::try_from(record.lifecycle.revision()).ok() != Some(self.revision)
            || record.lifecycle.retains_capacity() != self.retained
        {
            return Err(BackgroundTaskError::Storage);
        }
        let admitted = BackgroundRecord {
            lifecycle: TaskLifecycle::admitted(record.lifecycle.admitted_at_ms()),
            ..record.clone()
        };
        let expected: serde_json::Value =
            serde_json::from_str(&encoding::evidence(None, &admitted)?)
                .map_err(|_| BackgroundTaskError::Storage)?;
        let actual: serde_json::Value = serde_json::from_str(
            self.admission
                .as_deref()
                .ok_or(BackgroundTaskError::Storage)?,
        )
        .map_err(|_| BackgroundTaskError::Storage)?;
        if actual != expected {
            return Err(BackgroundTaskError::Storage);
        }
        Ok(record)
    }
}
