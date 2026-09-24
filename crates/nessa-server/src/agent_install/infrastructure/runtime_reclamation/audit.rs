use std::{
    io,
    path::{Path, PathBuf},
};

use nessa_local_storage::{
    create_directory, create_directory_beneath, open_beneath, sync_directory_beneath, OpenMode,
    PrivateTempFile,
};
use sha2::{Digest, Sha256};

use crate::agent_install::{
    application::{ReclamationAudit, ReclamationAuditFailure},
    domain::ReclamationEvent,
    infrastructure::runtime_reclamation::StoredEvent,
};

const RECORDS: &str = "records";
const LOCK: &str = "audit.lock";

pub struct DurableReclamationAudit {
    root: PathBuf,
}

impl DurableReclamationAudit {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ReclamationAuditFailure> {
        let root = root.into();
        create_directory(&root).map_err(audit_error)?;
        create_directory_beneath(&root, Path::new(RECORDS)).map_err(audit_error)?;
        sync_directory_beneath(&root, Path::new(RECORDS)).map_err(audit_error)?;
        Ok(Self { root })
    }

    fn record_path(event: &ReclamationEvent) -> PathBuf {
        let digest = Sha256::digest(event.admission().operation_id().as_str().as_bytes());
        Path::new(RECORDS).join(format!("{digest:x}.json"))
    }
}

impl ReclamationAudit for DurableReclamationAudit {
    fn record(&self, event: &ReclamationEvent) -> Result<(), ReclamationAuditFailure> {
        let lock_path = Path::new(RECORDS).join(LOCK);
        let lock =
            open_beneath(&self.root, &lock_path, OpenMode::OpenOrCreate).map_err(audit_error)?;
        lock.lock().map_err(audit_error)?;
        let destination = Self::record_path(event);
        match open_beneath(&self.root, &destination, OpenMode::ReadNonblocking) {
            Ok(file) => {
                let restored: StoredEvent = serde_json::from_reader(file)
                    .map_err(|error| ReclamationAuditFailure::new(error.to_string()))?;
                if restored.restore().map_err(ReclamationAuditFailure::new)? != *event {
                    return Err(ReclamationAuditFailure::new(
                        "reclamation operation identity has conflicting audit facts".into(),
                    ));
                }
                sync_directory_beneath(&self.root, Path::new(RECORDS)).map_err(audit_error)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut staging = PrivateTempFile::new_beneath(&self.root, Path::new(RECORDS))
                    .map_err(audit_error)?;
                serde_json::to_writer_pretty(
                    staging.as_file_mut(),
                    &StoredEvent::from_domain(event),
                )
                .map_err(|error| ReclamationAuditFailure::new(error.to_string()))?;
                staging.as_file().sync_all().map_err(audit_error)?;
                staging.persist_beneath(&destination).map_err(audit_error)?;
                sync_directory_beneath(&self.root, Path::new(RECORDS)).map_err(audit_error)
            }
            Err(error) => Err(audit_error(error)),
        }
    }
}

fn audit_error(error: io::Error) -> ReclamationAuditFailure {
    ReclamationAuditFailure::new(error.to_string())
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/reclamation_audit.rs"]
mod tests;
