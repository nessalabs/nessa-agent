//! Durable warm-up evidence, committed before a completion record is written.
use crate::agent_warm_up::application::{
    WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpFuture,
};
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::json;
use std::{io::Write, path::PathBuf};
use uuid::Uuid;

/// Writes one immutable record per warm-up attempt.
pub struct DurableWarmUpAudit {
    directory: PathBuf,
}

impl DurableWarmUpAudit {
    /// Open, creating the directory when it does not exist.
    ///
    /// # Errors
    /// Fails when the directory cannot be created.
    pub fn new(directory: PathBuf) -> Result<Self, WarmUpError> {
        create_directory(&directory).map_err(|error| WarmUpError::Audit(error.to_string()))?;
        Ok(Self { directory })
    }
}

impl WarmUpAudit for DurableWarmUpAudit {
    fn record(&self, record: WarmUpAuditRecord) -> WarmUpFuture<'_, ()> {
        let id = Uuid::new_v4().to_string();
        let value = json!({
            "recordId": id,
            "kind": "agent_runtime_warm_up",
            "target": {
                "executable": record.runtime.executable(),
                "entry": record.runtime.entry(),
                "model": record.runtime.model(),
                "sessionId": record.session_id,
            },
            "transition": {
                "before": record.before.as_str(),
                "after": record.after.as_str(),
            },
            // Nobody asked for this. The gateway launched a runtime it had
            // never launched before, and says so rather than naming a person.
            "cause": "automatic_runtime_warm_up",
            "initiator": {
                "principalId": "gateway",
                "surfaceId": "runtime_warm_up",
            },
            "failure": record.failure,
            "correlationId": record.correlation_id,
            "requestedAtMs": record.requested_at_ms,
            "observedAtMs": record.observed_at_ms,
        });
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let failed = |error: &dyn std::fmt::Display| WarmUpError::Audit(error.to_string());
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|error| failed(&error))?;
                serde_json::to_writer(file.as_file_mut(), &value)
                    .map_err(|error| failed(&error))?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|()| file.as_file().sync_all())
                    .map_err(|error| failed(&error))?;
                file.persist(&directory.join(format!("{id}.json")))
                    .map_err(|error| failed(&error))?;
                sync_directory(&directory).map_err(|error| failed(&error))
            })
            .await
            .map_err(|error| WarmUpError::Audit(error.to_string()))?
        })
    }
}
