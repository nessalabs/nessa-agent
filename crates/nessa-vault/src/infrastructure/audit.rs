use crate::{
    application::{Audit, AuditRecord, ObservationClock},
    contract,
    domain::{EntryName, Failure, Status},
};
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
};
/// Secret-free bounded journal, independently readable while the vault is locked.
pub struct FileAudit {
    directory: PathBuf,
    clock: Arc<dyn ObservationClock>,
}
impl FileAudit {
    pub fn new(directory: PathBuf, clock: Arc<dyn ObservationClock>) -> Self {
        Self { directory, clock }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Stored {
    target: Option<String>,
    sequence: u64,
    observed_at_unix_ms: u64,
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
        let observed_at_unix_ms = self.clock.unix_millis()?;
        if observed_at_unix_ms > 9_007_199_254_740_991 {
            return Err(Failure::AuditUnavailable);
        }
        rows.push(Stored {
            observed_at_unix_ms,
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
        validate(&rows)?;
        let keep = rows.len().saturating_sub(contract::AUDIT_ROWS);
        rows.drain(..keep);
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

/// Validate retained evidence before retention can discard or rewrite it. This
/// owns row semantics for both restored and newly appended observations. Durable
/// sequence orders writes; concurrent operations and UTC time need not be ordered.
fn validate(rows: &[Stored]) -> Result<(), Failure> {
    let mut identities = std::collections::BTreeMap::new();
    let mut operations = std::collections::BTreeSet::new();
    for (index, row) in rows.iter().enumerate() {
        let known_state = |state: &str| matches!(state, "unconfigured" | "locked" | "unlocked");
        if row.sequence == 0
            || (index > 0 && rows[index - 1].sequence.checked_add(1) != Some(row.sequence))
            || row.observed_at_unix_ms > 9_007_199_254_740_991
            || [&row.lease, &row.principal, &row.credential]
                .iter()
                .any(|id| id.is_empty() || id.len() > 256)
            || !known_state(&row.before)
            || !known_state(&row.after)
            || !operations.insert((&row.lease, row.operation))
        {
            return Err(Failure::AuditUnavailable);
        }
        let actor = (&row.principal, &row.credential);
        if identities
            .insert(&row.lease, actor)
            .is_some_and(|prior| prior != actor)
        {
            return Err(Failure::AuditUnavailable);
        }
        let valid = match row.action.as_str() {
            "setup" | "unlock" if row.target.is_none() => {
                match (row.cause.as_str(), row.result.as_str()) {
                    ("explicit_request", "requested") => row.before == row.after,
                    ("verified_receipt", "published") => row.after == "unlocked",
                    ("connection_closed" | "explicit_lock", "not_published") => true,
                    (
                        "verification_failed",
                        "wrong_passphrase_or_ciphertext"
                        | "already_configured"
                        | "capacity"
                        | "unavailable",
                    ) => true,
                    _ => false,
                }
            }
            "passphrase_change" if row.target.is_none() => {
                match (row.cause.as_str(), row.result.as_str()) {
                    ("explicit_request", "requested") => {
                        row.before == "unlocked" && row.after == "unlocked"
                    }
                    ("encrypted_receipt", "committed")
                    | ("storage_refused" | "audit_refused", "unchanged")
                    | ("storage_unavailable", "not_confirmed") => {
                        row.before == "locked" && row.after == "locked"
                    }
                    _ => false,
                }
            }
            "lock" => {
                row.target.is_none()
                    && row.before == "unlocked"
                    && row.after == "locked"
                    && matches!(row.cause.as_str(), "explicit_request" | "connection_closed")
                    && row.result == "key_cleared"
            }
            "entry_saved" | "entry_deleted" => {
                row.target
                    .as_ref()
                    .is_some_and(|target| EntryName::new(target.as_str()).is_ok())
                    && row.before == "unlocked"
                    && row.after == "unlocked"
                    && matches!(
                        (row.cause.as_str(), row.result.as_str()),
                        ("internal_consumer", "requested") | ("encrypted_receipt", "committed")
                    )
            }
            _ => false,
        };
        if !valid {
            return Err(Failure::AuditUnavailable);
        }
    }
    Ok(())
}
