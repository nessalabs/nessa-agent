use std::{
    ffi::OsStr,
    io,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use nessa_local_storage::{
    create_directory, create_directory_beneath, sync_directory_beneath, OpenMode, PrivateDirectory,
};
use sha2::{Digest, Sha256};

use crate::agent_install::{
    application::{ReclamationAudit, ReclamationAuditFailure},
    domain::{ReclamationEvent, ReclamationOperationId},
    infrastructure::runtime_reclamation::StoredEvent,
};

const RECORDS: &str = "records";
const LOCK: &str = "audit.lock";
const MAXIMUM_AUDIT_RECORD_BYTES: u64 = 64 * 1024;

struct BoundedAuditBuffer {
    bytes: Vec<u8>,
}

impl BoundedAuditBuffer {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(MAXIMUM_AUDIT_RECORD_BYTES as usize),
        }
    }
}

impl Write for BoundedAuditBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|length| *length <= MAXIMUM_AUDIT_RECORD_BYTES as usize)
            .ok_or_else(|| io::Error::other("reclamation audit record exceeds its size bound"))?;
        self.bytes.reserve(length - self.bytes.len());
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct DurableReclamationAudit {
    records_authority: PrivateDirectory,
}

impl DurableReclamationAudit {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ReclamationAuditFailure> {
        let root = root.into();
        create_directory(&root).map_err(audit_error)?;
        create_directory_beneath(&root, Path::new(RECORDS)).map_err(audit_error)?;
        sync_directory_beneath(&root, Path::new(RECORDS)).map_err(audit_error)?;
        sync_directory_beneath(&root, Path::new("")).map_err(audit_error)?;
        let records_authority =
            PrivateDirectory::open_beneath(&root, Path::new(RECORDS)).map_err(audit_error)?;
        Ok(Self { records_authority })
    }

    fn record_path(event: &ReclamationEvent) -> PathBuf {
        Self::operation_path(event.admission().operation_id())
    }

    fn operation_path(operation_id: &ReclamationOperationId) -> PathBuf {
        let digest = Sha256::digest(operation_id.as_str().as_bytes());
        Path::new(RECORDS).join(format!("{digest:x}.json"))
    }

    fn operation_name(operation_id: &ReclamationOperationId) -> String {
        let digest = Sha256::digest(operation_id.as_str().as_bytes());
        format!("{digest:x}.json")
    }
}

impl DurableReclamationAudit {
    fn record_with(
        &self,
        event: &ReclamationEvent,
        replay_file_durable: impl Fn(&std::fs::File) -> io::Result<()>,
        directory_durable: impl Fn() -> io::Result<()>,
    ) -> Result<(), ReclamationAuditFailure> {
        self.verify_authority()?;
        let lock = self
            .records_authority
            .open_file(OsStr::new(LOCK), OpenMode::OpenOrCreate)
            .map_err(audit_error)?;
        lock.lock().map_err(audit_error)?;
        self.verify_lock(&lock)?;
        let destination = Self::operation_name(event.admission().operation_id());
        match self
            .records_authority
            .open_file(OsStr::new(&destination), OpenMode::ReadNonblocking)
        {
            Ok(mut file) => {
                let restored: StoredEvent = read_bounded_event(&mut file)?;
                if restored.restore().map_err(ReclamationAuditFailure::new)? != *event {
                    return Err(ReclamationAuditFailure::new(
                        "reclamation operation identity has conflicting audit facts".into(),
                    ));
                }
                replay_file_durable(&file).map_err(audit_error)?;
                directory_durable().map_err(audit_error)?;
                self.verify_lock(&lock)?;
                self.verify_authority()
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut staging = self.records_authority.reserve_temp().map_err(audit_error)?;
                let mut encoded = BoundedAuditBuffer::new();
                serde_json::to_writer_pretty(&mut encoded, &StoredEvent::from_domain(event))
                    .map_err(|error| ReclamationAuditFailure::new(error.to_string()))?;
                staging
                    .as_file_mut()
                    .write_all(&encoded.bytes)
                    .map_err(audit_error)?;
                match staging.publish_new(OsStr::new(&destination)) {
                    Ok(published) => {
                        replay_file_durable(published.as_file()).map_err(audit_error)?
                    }
                    Err(error)
                        if error.source_error().kind() == io::ErrorKind::AlreadyExists
                            && error.published().is_none() =>
                    {
                        let mut file = self
                            .records_authority
                            .open_file(OsStr::new(&destination), OpenMode::ReadNonblocking)
                            .map_err(audit_error)?;
                        let restored = read_bounded_event(&mut file)?
                            .restore()
                            .map_err(ReclamationAuditFailure::new)?;
                        if restored != *event {
                            return Err(ReclamationAuditFailure::new(
                                "reclamation operation identity has conflicting audit facts".into(),
                            ));
                        }
                        replay_file_durable(&file).map_err(audit_error)?;
                    }
                    Err(error) => {
                        return Err(ReclamationAuditFailure::new(error.to_string()));
                    }
                }
                directory_durable().map_err(audit_error)?;
                self.verify_lock(&lock)?;
                self.verify_authority()
            }
            Err(error) => Err(audit_error(error)),
        }
    }
}

impl ReclamationAudit for DurableReclamationAudit {
    fn record(&self, event: &ReclamationEvent) -> Result<(), ReclamationAuditFailure> {
        self.record_with(event, std::fs::File::sync_all, || {
            self.records_authority.sync()
        })
    }

    fn event_for(
        &self,
        operation_id: &ReclamationOperationId,
    ) -> Result<Option<ReclamationEvent>, ReclamationAuditFailure> {
        self.verify_authority()?;
        let lock = self
            .records_authority
            .open_file(OsStr::new(LOCK), OpenMode::OpenOrCreate)
            .map_err(audit_error)?;
        lock.lock().map_err(audit_error)?;
        self.verify_lock(&lock)?;
        let name = Self::operation_name(operation_id);
        let mut file = match self
            .records_authority
            .open_file(OsStr::new(&name), OpenMode::ReadNonblocking)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.verify_lock(&lock)?;
                self.verify_authority()?;
                return Ok(None);
            }
            Err(error) => return Err(audit_error(error)),
        };
        let event = read_bounded_event(&mut file)?;
        let event = event.restore().map_err(ReclamationAuditFailure::new)?;
        if event.admission().operation_id() != operation_id {
            return Err(ReclamationAuditFailure::new(
                "reclamation audit path contains another operation identity".into(),
            ));
        }
        file.sync_all().map_err(audit_error)?;
        self.verify_lock(&lock)?;
        self.verify_authority()?;
        Ok(Some(event))
    }
}

impl DurableReclamationAudit {
    fn verify_authority(&self) -> Result<(), ReclamationAuditFailure> {
        self.records_authority.verify_binding().map_err(audit_error)
    }

    fn verify_lock(&self, lock: &std::fs::File) -> Result<(), ReclamationAuditFailure> {
        if self
            .records_authority
            .named_file_is(OsStr::new(LOCK), lock)
            .map_err(audit_error)?
        {
            Ok(())
        } else {
            Err(ReclamationAuditFailure::new(
                "reclamation audit lock was replaced after acquisition".into(),
            ))
        }
    }
}

fn read_bounded_event(file: &mut std::fs::File) -> Result<StoredEvent, ReclamationAuditFailure> {
    if file.metadata().map_err(audit_error)?.len() > MAXIMUM_AUDIT_RECORD_BYTES {
        return Err(ReclamationAuditFailure::new(
            "reclamation audit record exceeds its size bound".into(),
        ));
    }
    let mut encoded = Vec::new();
    file.by_ref()
        .take(MAXIMUM_AUDIT_RECORD_BYTES + 1)
        .read_to_end(&mut encoded)
        .map_err(audit_error)?;
    if encoded.len() as u64 > MAXIMUM_AUDIT_RECORD_BYTES {
        return Err(ReclamationAuditFailure::new(
            "reclamation audit record grew past its size bound".into(),
        ));
    }
    serde_json::from_slice(&encoded)
        .map_err(|error| ReclamationAuditFailure::new(error.to_string()))
}

fn audit_error(error: io::Error) -> ReclamationAuditFailure {
    ReclamationAuditFailure::new(error.to_string())
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/reclamation_audit.rs"]
mod tests;
