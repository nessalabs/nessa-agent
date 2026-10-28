//! Durable execution evidence. Each call commits one private record before acknowledgement.
//! One instance worker bounds physical I/O through observer loss and captured cleanup.
use super::{
    audit_diagnostics::{observe_io, unavailable},
    audit_mapping::record_value,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{
    create_directory, physical_operation::Worker, sync_directory, PrivateTempFile,
};
use nessa_sdk::application::agent_execution::{
    agents::{AgentError, AgentFuture},
    executions::{ExecutionAudit, ExecutionAuditRecord},
};
use serde_json::json;
use std::{
    io::{Result as IoResult, Write},
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
}

struct PrivateExecutionAuditFileWriter;

impl ExecutionAuditFileWriter for PrivateExecutionAuditFileWriter {
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
        let directory = path.parent().ok_or_else(|| failure("parent"))?;
        let commit = || -> IoResult<()> {
            let mut file = observe_io(
                "execution",
                "create_temp",
                PrivateTempFile::new_in(directory),
            )?;
            observe_io(
                "execution",
                "write_record",
                file.as_file_mut().write_all(bytes),
            )?;
            observe_io(
                "execution",
                "write_newline",
                file.as_file_mut().write_all(b"\n"),
            )?;
            observe_io("execution", "sync_file", file.as_file().sync_all())?;
            observe_io("execution", "persist", file.persist(path))?;
            observe_io("execution", "sync_directory", sync_directory(directory))
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
        observe_io(
            "execution",
            "create_directory",
            create_directory(&directory),
        )
        .map_err(|_| AgentError::AuditFailure)?;
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
                .map_err(|_| failure("admission"))?;
            let state = self.state.clone();
            admission
                .submit(move || {
                    let id = Uuid::new_v4().to_string();
                    let _span = tracing::error_span!(
                        "execution_audit", record_id = %id, writer_id = %state.writer_id
                    )
                    .entered();
                    let value = json!({
                        "recordId": id,
                        "writerId": state.writer_id,
                        "sequence": state.sequence.fetch_add(1, Ordering::SeqCst),
                        "observedAtMs": state.clock.unix_milliseconds(),
                        "record": record_value(&record),
                    });
                    let bytes = serde_json::to_vec(&value).map_err(|_| failure("encode"))?;
                    state
                        .writer
                        .write(&state.directory.join(format!("{id}.json")), &bytes)
                        .map_err(|_| failure("writer"))
                })
                .await
                .map_err(|_| failure("worker"))?
        })
    }
}

fn failure(phase: &'static str) -> AgentError {
    unavailable("execution", phase);
    AgentError::AuditFailure
}

#[cfg(test)]
#[path = "../../../tests/conversation/audit.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/conversation/audit_diagnostics.rs"]
mod diagnostics_tests;
