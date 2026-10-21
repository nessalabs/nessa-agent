//! Durable execution evidence. Each call commits one private record before acknowledgement.
//! One instance worker bounds physical I/O through observer loss and captured cleanup.
use super::audit_mapping::record_value;
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{
    create_directory, open, physical_operation::Worker, sync_directory, OpenMode, PrivateTempFile,
};
use nessa_sdk::application::agent_execution::{
    agents::{AgentError, AgentFuture},
    executions::{ExecutionAudit, ExecutionAuditRecord, TurnResumption},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{ErrorKind, Read, Result as IoResult, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use uuid::Uuid;

/// Synchronous private-file persistence boundary for execution audit evidence.
///
/// Called on the adapter's admitted physical worker. Success acknowledges durable
/// publication of `bytes` at `path`, including the containing directory. Caller
/// loss does not stop a call already submitted to the worker.
pub trait ExecutionAuditFileWriter: Send + Sync {
    /// Publish one record. Failure may follow a physical effect; it is not rollback.
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), AgentError>;

    /// Publish a stable prepared decision identity exactly once, or acknowledge its exact
    /// existing semantic payload after syncing the winner. Never replace an
    /// existing record. Unsupported injected writers fail conservatively.
    fn write_once(&self, _path: &Path, _bytes: &[u8]) -> Result<(), AgentError> {
        Err(AgentError::AuditFailure)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeclineAuditReceipt {
    record_id: String,
    writer_id: String,
    #[serde(rename = "sequence")]
    _sequence: u64,
    #[serde(rename = "observedAtMs")]
    _observed_at_ms: u64,
    record: Box<serde_json::value::RawValue>,
}

struct PrivateExecutionAuditFileWriter;

impl ExecutionAuditFileWriter for PrivateExecutionAuditFileWriter {
    fn write_once(&self, path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
        let directory = path.parent().ok_or(AgentError::AuditFailure)?;
        let expected: DeclineAuditReceipt =
            serde_json::from_slice(bytes).map_err(|_| AgentError::AuditFailure)?;
        let acknowledge = || -> Result<(), AgentError> {
            let mut file = open(path, OpenMode::ReadWrite).map_err(|_| AgentError::AuditFailure)?;
            let bound = bytes.len().saturating_add(4096);
            if file.metadata().map_err(|_| AgentError::AuditFailure)?.len() > bound as u64 {
                return Err(AgentError::AuditFailure);
            }
            let mut stored = Vec::new();
            (&mut file)
                .take(bound as u64 + 1)
                .read_to_end(&mut stored)
                .map_err(|_| AgentError::AuditFailure)?;
            let value: DeclineAuditReceipt =
                serde_json::from_slice(&stored).map_err(|_| AgentError::AuditFailure)?;
            // Our writer uses one canonical encoding for semantic payloads.
            // Comparing its raw payload also rejects duplicate nested keys.
            if value.record.get() != expected.record.get()
                || value.record_id != expected.record_id
                || Uuid::parse_str(&value.writer_id).is_err()
            {
                return Err(AgentError::AuditFailure);
            }
            // Visible matching bytes alone cannot acknowledge a lost sync reply.
            file.sync_all().map_err(|_| AgentError::AuditFailure)?;
            sync_directory(directory).map_err(|_| AgentError::AuditFailure)
        };
        match open(path, OpenMode::ReadNonblocking) {
            Ok(_) => return acknowledge(),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(AgentError::AuditFailure),
        }
        let mut file = PrivateTempFile::new_in(directory).map_err(|_| AgentError::AuditFailure)?;
        file.as_file_mut()
            .write_all(bytes)
            .map_err(|_| AgentError::AuditFailure)?;
        file.as_file_mut()
            .write_all(b"\n")
            .map_err(|_| AgentError::AuditFailure)?;
        file.as_file()
            .sync_all()
            .map_err(|_| AgentError::AuditFailure)?;
        match file.publish(path) {
            Ok(()) => sync_directory(directory).map_err(|_| AgentError::AuditFailure),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => acknowledge(),
            Err(_) => Err(AgentError::AuditFailure),
        }
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
        let directory = path.parent().ok_or(AgentError::AuditFailure)?;
        let commit = || -> IoResult<()> {
            let mut file = PrivateTempFile::new_in(directory)?;
            file.as_file_mut().write_all(bytes)?;
            file.as_file_mut().write_all(b"\n")?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            sync_directory(directory)
        };
        commit().map_err(|_| AgentError::AuditFailure)
    }
}

/// Private host audit storage, separate from SDK conversation records and UI views.
/// Successful acknowledgement means the record file and directory were synced.
///
/// One instance admits one queued/executing physical write. Later observers wait
/// asynchronously and can drop before admission without submitting a write
/// (`dropped_audit_observers_do_not_grow_physical_work`). Started writes keep
/// admission through captured cleanup, even after observer loss. This is not a
/// path-wide lock or a shutdown drain; the originating runtime must remain alive.
pub struct DurableExecutionAudit {
    state: Arc<AuditState>,
    worker: Worker,
}

struct AuditState {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
    writer: Arc<dyn ExecutionAuditFileWriter>,
    writer_id: String,
    sequence: AtomicU64,
}

impl DurableExecutionAudit {
    /// Use a private `directory` and the injected absolute `clock` for observations.
    /// Existing unsafe paths fail initialization; they are never silently repaired.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, AgentError> {
        Self::with_writer(directory, clock, Arc::new(PrivateExecutionAuditFileWriter))
    }

    /// Inject the physical private-file boundary, retaining this adapter's admission.
    /// The writer is called after admission, on a blocking worker, with a complete
    /// encoded record. Metadata is sampled when the physical job starts. Waiting
    /// observers own their input but allocate no encoded copy before admission.
    pub fn with_writer(
        directory: PathBuf,
        clock: Arc<dyn Clock>,
        writer: Arc<dyn ExecutionAuditFileWriter>,
    ) -> Result<Self, AgentError> {
        create_directory(&directory).map_err(|_| AgentError::AuditFailure)?;
        Ok(Self {
            state: Arc::new(AuditState {
                directory,
                clock,
                writer,
                writer_id: Uuid::new_v4().to_string(),
                sequence: AtomicU64::new(0),
            }),
            worker: Worker::new(),
        })
    }
}

impl ExecutionAudit for DurableExecutionAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            let admission = self
                .worker
                .admit()
                .await
                .map_err(|_| AgentError::AuditFailure)?;
            let state = self.state.clone();
            admission
                .submit(move || {
                    let stable = match &record {
                        ExecutionAuditRecord::TurnResumed(record)
                            if record.resumption() == &TurnResumption::Declined =>
                        {
                            let key = serde_json::to_vec(&(
                                record.session_id().as_str(),
                                record.execution_id().as_str(),
                            ))
                            .map_err(|_| AgentError::AuditFailure)?;
                            Some(format!("decline-{:x}", Sha256::digest(key)))
                        }
                        ExecutionAuditRecord::QueueSettled(record)
                            if record.after() == nessa_sdk::domain::agent_execution::executions::InvocationStage::Cancelled =>
                        {
                            let key = serde_json::to_vec(&(record.session_id().as_str(), record.execution_id().as_str()))
                                .map_err(|_| AgentError::AuditFailure)?;
                            Some(format!("cancellation-{:x}", Sha256::digest(key)))
                        }
                        _ => None,
                    };
                    let id = stable.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
                    let value = json!({
                        "recordId": id,
                        "writerId": state.writer_id,
                        "sequence": state.sequence.fetch_add(1, Ordering::SeqCst),
                        "observedAtMs": state.clock.unix_milliseconds(),
                        "record": record_value(&record),
                    });
                    let bytes = serde_json::to_vec(&value).map_err(|_| AgentError::AuditFailure)?;
                    let path = state.directory.join(format!("{id}.json"));
                    match stable {
                        Some(_) => state.writer.write_once(&path, &bytes),
                        None => state.writer.write(&path, &bytes),
                    }
                    .map_err(|_| AgentError::AuditFailure)
                })
                .await
                .map_err(|_| AgentError::AuditFailure)?
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/audit.rs"]
mod tests;
