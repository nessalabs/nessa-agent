use crate::{
    application::{Audit, AuditRecord},
    contract,
    domain::{Failure, Status},
};
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
};
/// Secret-free bounded journal, independently readable while the vault is locked.
pub struct FileAudit {
    directory: PathBuf,
}
impl FileAudit {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Stored {
    target: Option<String>,
    sequence: u64,
    operation: u64,
    lease: String,
    principal: String,
    credential: String,
    action: String,
    before: String,
    after: String,
    cause: String,
    result: String,
}
impl Audit for FileAudit {
    fn record(&self, record: &AuditRecord) -> Result<(), Failure> {
        create_directory(&self.directory).map_err(problem)?;
        let lock =
            open(&self.directory.join("audit.lock"), OpenMode::OpenOrCreate).map_err(problem)?;
        lock.lock().map_err(problem)?;
        let path = self.directory.join("events.json");
        let mut rows: Vec<Stored> = match open(&path, OpenMode::Read) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(2_097_153)
                    .read_to_end(&mut bytes)
                    .map_err(problem)?;
                if bytes.len() > 2_097_152 {
                    return Err(Failure::AuditUnavailable);
                }
                serde_json::from_slice(&bytes).map_err(|_| Failure::AuditUnavailable)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return Err(Failure::AuditUnavailable),
        };
        if rows.len() > contract::AUDIT_ROWS {
            return Err(Failure::AuditUnavailable);
        }
        let sequence = rows
            .last()
            .map_or(Some(1), |row| row.sequence.checked_add(1))
            .ok_or(Failure::AuditUnavailable)?;
        if record.lease.len() > 256
            || record.caller.principal.len() > 256
            || record.caller.credential.len() > 256
        {
            return Err(Failure::AuditUnavailable);
        }
        let keep = rows.len().saturating_sub(contract::AUDIT_ROWS - 1);
        rows.drain(..keep);
        rows.push(Stored {
            target: record.target.as_ref().map(|name| name.as_str().to_owned()),
            sequence,
            operation: record.operation,
            lease: record.lease.clone(),
            principal: record.caller.principal.clone(),
            credential: record.caller.credential.clone(),
            action: record.action.into(),
            before: state(record.before).into(),
            after: state(record.after).into(),
            cause: record.cause.into(),
            result: record.result.into(),
        });
        let bytes = serde_json::to_vec(&rows).map_err(|_| Failure::AuditUnavailable)?;
        if bytes.len() > 2_097_152 {
            return Err(Failure::AuditUnavailable);
        }
        let mut temp = PrivateTempFile::new_in(&self.directory).map_err(problem)?;
        temp.as_file_mut().write_all(&bytes).map_err(problem)?;
        temp.as_file().sync_all().map_err(problem)?;
        temp.persist(&path).map_err(problem)?;
        sync_directory(&self.directory).map_err(problem)
    }
}
fn problem(_: std::io::Error) -> Failure {
    Failure::AuditUnavailable
}
fn state(status: Status) -> &'static str {
    match status {
        Status::Unconfigured => "unconfigured",
        Status::Locked => "locked",
        Status::Unlocked => "unlocked",
    }
}
