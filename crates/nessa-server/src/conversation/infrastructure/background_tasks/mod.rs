//! Private task records and transition audit in one SQLite transaction.
//! `BackgroundStore -> tasks (current output) + transitions (minimal evidence)`.
mod encoding;
mod rows;
#[cfg(unix)]
mod runner;
#[cfg(unix)]
pub use runner::ShepherdBackgroundTasks;

use crate::conversation::application::background_tasks::{
    BackgroundRecord, BackgroundRun, BackgroundStore, BackgroundTaskError, MAX_BACKGROUND_TASKS,
    MAX_CONVERSATION_BACKGROUND_TASKS,
};
use crate::conversation::domain::{BackgroundTaskId, TaskInitiator};
use nessa_local_database::{
    rusqlite::{params, Connection, OptionalExtension},
    OpenError, Schema,
};
use nessa_protocol::conversation::domain::ConversationId;
use std::{
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

pub struct LocalBackgroundTasks {
    connection: Arc<Mutex<Connection>>,
}
impl LocalBackgroundTasks {
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let connection =
            nessa_local_database::open(path, &Schema::new(include_str!("schema.sql"))?)?;
        // Validate all current rows once before indexed recovery can omit a
        // corrupted retained flag. Stream history rather than collecting it.
        {
            let mut statement = connection.prepare(rows::SELECT)?;
            let stored = statement.query_map([], rows::StoredRow::read)?;
            for row in stored {
                row?.decode().map_err(|_| {
                    OpenError::Damaged(
                        "background task row disagrees with its indexed or admission evidence"
                            .into(),
                    )
                })?;
            }
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }
    fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, BackgroundTaskError> + Send + 'static,
    ) -> BackgroundRun<'_, Result<T, BackgroundTaskError>> {
        let connection = self.connection.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                work(&mut connection.lock().unwrap_or_else(PoisonError::into_inner))
            })
            .await
            .map_err(|_| BackgroundTaskError::Storage)?
        })
    }
}
fn failed(error: nessa_local_database::rusqlite::Error) -> BackgroundTaskError {
    tracing::warn!(%error, "background task store failed");
    BackgroundTaskError::Storage
}
fn read(
    connection: &Connection,
    conversation: &str,
    task: &str,
) -> Result<Option<BackgroundRecord>, BackgroundTaskError> {
    connection
        .query_row(
            &format!("{} WHERE t.conversation = ?1 AND t.task = ?2", rows::SELECT),
            params![conversation, task],
            rows::StoredRow::read,
        )
        .optional()
        .map_err(failed)?
        .map(rows::StoredRow::decode)
        .transpose()
}
impl BackgroundStore for LocalBackgroundTasks {
    fn read(
        &self,
        conversation: &ConversationId,
        task: &BackgroundTaskId,
    ) -> BackgroundRun<'_, Result<Option<BackgroundRecord>, BackgroundTaskError>> {
        let conversation = conversation.to_string();
        let task = task.as_str().to_owned();
        self.run(move |db| read(db, &conversation, &task))
    }
    fn save(
        &self,
        record: BackgroundRecord,
        previous: Option<u64>,
    ) -> BackgroundRun<'_, Result<(), BackgroundTaskError>> {
        self.run(move |db| {
            let transaction = db.transaction().map_err(failed)?;
            let conversation = record.source.conversation.to_string();
            let old = read(&transaction, &conversation, record.id.as_str())?;
            if old.as_ref() == Some(&record) { return Ok(()); }
            if old.as_ref().map(|r| r.lifecycle.revision()) != previous { return Err(BackgroundTaskError::Conflict); }
            match &old {
                Some(old) if old.source != record.source || old.command != record.command
                    || !record.lifecycle.follows(&old.lifecycle) => return Err(BackgroundTaskError::Conflict),
                None => {
                    if record.lifecycle.revision() != 0 { return Err(BackgroundTaskError::Conflict); }
                    let (total, local): (i64, i64) = transaction.query_row("SELECT COUNT(*), COALESCE(SUM(conversation = ?1), 0) FROM tasks WHERE retained = 1", [&conversation], |row| Ok((row.get(0)?, row.get(1)?))).map_err(failed)?;
                    if total >= MAX_BACKGROUND_TASKS as i64 || local >= MAX_CONVERSATION_BACKGROUND_TASKS as i64 { return Err(BackgroundTaskError::Capacity); }
                }
                _ => (),
            }
            transaction.execute("INSERT INTO tasks (conversation, task, execution, tool_call, revision, retained, record) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(conversation, task) DO UPDATE SET revision = excluded.revision, retained = excluded.retained, record = excluded.record", params![conversation, record.id.as_str(), record.source.execution, record.source.tool_call, i64::try_from(record.lifecycle.revision()).map_err(|_| BackgroundTaskError::Storage)?, record.lifecycle.retains_capacity(), encoding::encode(&record)?]).map_err(failed)?;
            transaction.execute("INSERT INTO transitions (conversation, task, revision, evidence) VALUES (?1, ?2, ?3, ?4)", params![conversation, record.id.as_str(), i64::try_from(record.lifecycle.revision()).map_err(|_| BackgroundTaskError::Storage)?, encoding::evidence(old.as_ref(), &record)?]).map_err(failed)?;
            transaction.commit().map_err(failed)
        })
    }
    fn forget_conversation(
        &self,
        conversation: &ConversationId,
        initiator: TaskInitiator,
    ) -> BackgroundRun<'_, Result<(), BackgroundTaskError>> {
        let conversation = conversation.to_string();
        self.run(move |db| {
            let transaction = db.transaction().map_err(failed)?;
            {
                let mut statement = transaction.prepare(&format!("{} WHERE t.conversation = ?1", rows::SELECT)).map_err(failed)?;
                let rows = statement.query_map([&conversation], rows::StoredRow::read).map_err(failed)?;
                for row in rows {
                    let record = row.map_err(failed)?.decode()?;
                    if record.lifecycle.retains_capacity() { return Err(BackgroundTaskError::CleanupUnverified); }
                    let revision = record.lifecycle.revision().checked_add(1).and_then(|r| i64::try_from(r).ok()).ok_or(BackgroundTaskError::Storage)?;
                    transaction.execute("INSERT INTO transitions (conversation, task, revision, evidence) VALUES (?1, ?2, ?3, ?4)", params![conversation, record.id.as_str(), revision, encoding::erasure_evidence(&record, &initiator)]).map_err(failed)?;
                }
            }
            transaction.execute("DELETE FROM tasks WHERE conversation = ?1", [&conversation]).map_err(failed)?;
            transaction.commit().map_err(failed)
        })
    }
    fn pending(&self) -> BackgroundRun<'_, Result<Vec<BackgroundRecord>, BackgroundTaskError>> {
        self.run(|db| {
            let mut statement = db
                .prepare(&format!("{} WHERE t.retained = 1 LIMIT ?1", rows::SELECT))
                .map_err(failed)?;
            let rows = statement
                .query_map([(MAX_BACKGROUND_TASKS + 1) as i64], rows::StoredRow::read)
                .map_err(failed)?;
            let records = rows
                .map(|row| row.map_err(failed)?.decode())
                .collect::<Result<Vec<_>, _>>()?;
            if records.len() > MAX_BACKGROUND_TASKS {
                return Err(BackgroundTaskError::Storage);
            }
            Ok(records)
        })
    }
    fn for_calls(
        &self,
        conversation: &ConversationId,
        calls: Vec<(String, String)>,
    ) -> BackgroundRun<'_, Result<Vec<BackgroundRecord>, BackgroundTaskError>> {
        let conversation = conversation.to_string();
        self.run(move |db| {
            let mut statement = db
                .prepare(&format!(
                    "{} WHERE t.conversation = ?1 AND t.execution = ?2 AND t.tool_call = ?3",
                    rows::SELECT
                ))
                .map_err(failed)?;
            let mut records = Vec::new();
            for (execution, call) in calls {
                if let Some(value) = statement
                    .query_row(
                        params![conversation, execution, call],
                        rows::StoredRow::read,
                    )
                    .optional()
                    .map_err(failed)?
                {
                    records.push(value.decode()?);
                }
            }
            Ok(records)
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/background_store.rs"]
mod tests;
