//! Durable execution evidence. Each call commits one private record before acknowledgement.
//! Separate files avoid torn shared appends and preserve concurrent records independently.
use super::audit_mapping::record_value;
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use nessa_sdk::application::agent_execution::{
    agents::{AgentError, AgentFuture},
    executions::{ExecutionAudit, ExecutionAuditRecord},
};
use serde_json::json;
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use uuid::Uuid;

/// Private host audit storage, separate from the SDK session journal and UI views.
/// Successful acknowledgement means the record file and directory were synced.
pub struct DurableExecutionAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
    writer_id: String,
    sequence: AtomicU64,
}
impl DurableExecutionAudit {
    /// Use a private `directory` and the injected absolute `clock` for observations.
    /// Existing unsafe paths fail initialization; they are never silently repaired.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, AgentError> {
        create_directory(&directory).map_err(|_| AgentError::AuditFailure)?;
        Ok(Self {
            directory,
            clock,
            writer_id: Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
        })
    }
}
impl ExecutionAudit for DurableExecutionAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let id = Uuid::new_v4().to_string();
        let value = json!({"recordId":id,"writerId":self.writer_id,"sequence":self.sequence.fetch_add(1, Ordering::SeqCst),"observedAtMs":self.clock.unix_milliseconds(),"record":record_value(&record)});
        let directory = self.directory.clone();
        Box::pin(async move {
            // The blocking task owns the write even if its async caller times out.
            // An uncertain acknowledgement never causes physical cleanup to stop.
            tokio::task::spawn_blocking(move || {
                let mut file = PrivateTempFile::new_in(&directory)?;
                serde_json::to_writer(file.as_file_mut(), &value)?;
                file.as_file_mut().write_all(b"\n")?;
                file.as_file().sync_all()?;
                file.persist(&directory.join(format!("{id}.json")))?;
                sync_directory(&directory)
            })
            .await
            .map_err(|_| AgentError::AuditFailure)?
            .map_err(|_| AgentError::AuditFailure)
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/audit.rs"]
mod tests;
