//! Durable install evidence: one private, monotonically sequenced file per
//! transition. A filesystem lock serializes writers across CLI processes;
//! clocks describe observation time and never decide effect order.
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use nessa_auth::application::ports::Clock;
use nessa_local_storage::{
    create_durable_directory_beneath, open, open_beneath, sync_directory_beneath, OpenMode,
    PrivateTempFile,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent_install::{
    application::{AuditFailure, InstallAudit},
    domain::{
        InstallFailureEvidence, InstallFailureKind, InstallTransition, InstallTransitionKind,
        RecoveryState, RollbackState, RuntimeArtifact,
    },
};

/// Private host audit storage for agent-runtime installation transitions.
pub struct DurableInstallAudit {
    root: PathBuf,
    directory: PathBuf,
    lock_path: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableInstallAudit {
    pub fn new(root: &Path, directory: &Path, clock: Arc<dyn Clock>) -> Result<Self, AuditFailure> {
        create_durable_directory_beneath(root, directory).map_err(audit_failure)?;
        let lock_path = directory.join("audit.lock");
        open_beneath(root, &lock_path, OpenMode::OpenOrCreate).map_err(audit_failure)?;
        sync_directory_beneath(root, directory).map_err(audit_failure)?;
        Ok(Self {
            root: root.to_owned(),
            directory: directory.to_owned(),
            lock_path,
            clock,
        })
    }
}

impl InstallAudit for DurableInstallAudit {
    fn record(&self, transition: InstallTransition) -> Result<(), AuditFailure> {
        let lock = open_beneath(&self.root, &self.lock_path, OpenMode::OpenOrCreate)
            .map_err(audit_failure)?;
        lock.lock().map_err(audit_failure)?;
        let sequence = next_sequence(&self.root.join(&self.directory))?;
        let id = Uuid::new_v4().to_string();
        let mut value = record_value(&transition);
        value["recordId"] = json!(id);
        value["sequence"] = json!(sequence);
        value["observedAtMs"] = json!(self.clock.unix_milliseconds());
        let mut file =
            PrivateTempFile::new_beneath(&self.root, &self.directory).map_err(audit_failure)?;
        serde_json::to_writer(file.as_file_mut(), &value)
            .map_err(|error| AuditFailure(error.to_string()))?;
        file.as_file_mut().write_all(b"\n").map_err(audit_failure)?;
        file.as_file().sync_all().map_err(audit_failure)?;
        file.persist_beneath(&self.directory.join(format!("{sequence:020}.json")))
            .map_err(audit_failure)?;
        sync_directory_beneath(&self.root, &self.directory).map_err(audit_failure)
    }
}

fn next_sequence(directory: &Path) -> Result<u64, AuditFailure> {
    let mut sequences = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(audit_failure)? {
        let entry = entry.map_err(audit_failure)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(number) = name.strip_suffix(".json") else {
            continue;
        };
        let sequence = number
            .parse::<u64>()
            .map_err(|_| AuditFailure(format!("invalid audit record name {name}")))?;
        let mut record = open(&entry.path(), OpenMode::ReadNonblocking).map_err(audit_failure)?;
        let mut encoded = Vec::new();
        record.read_to_end(&mut encoded).map_err(audit_failure)?;
        let value: Value = serde_json::from_slice(&encoded)
            .map_err(|error| AuditFailure(format!("invalid audit record {name}: {error}")))?;
        if value["sequence"].as_u64() != Some(sequence) {
            return Err(AuditFailure(format!(
                "audit record {name} disagrees with its sequence"
            )));
        }
        sequences.push(sequence);
    }
    sequences.sort_unstable();
    for (index, sequence) in sequences.iter().enumerate() {
        let expected = u64::try_from(index).unwrap_or(u64::MAX) + 1;
        if *sequence != expected {
            return Err(AuditFailure(format!(
                "audit sequence is not contiguous at {expected}"
            )));
        }
    }
    u64::try_from(sequences.len())
        .ok()
        .and_then(|last| last.checked_add(1))
        .ok_or_else(|| AuditFailure("audit sequence exhausted".into()))
}

fn artifact(value: &RuntimeArtifact) -> Value {
    json!({
        "version": value.version().as_str(),
        "digest": value.digest().as_str(),
        "executable": value.executable().as_str(),
    })
}

fn installed(value: &RuntimeArtifact) -> Value {
    json!({"state": "installed", "artifact": artifact(value)})
}

fn failure(value: &InstallFailureEvidence) -> Value {
    let kind = match value.kind() {
        InstallFailureKind::Unwritable => "unwritable",
        InstallFailureKind::Unreadable => "unreadable",
        InstallFailureKind::MissingExecutable => "missing_executable",
        InstallFailureKind::MalformedArchive => "malformed_archive",
    };
    json!({"kind": kind, "detail": value.detail()})
}

fn caller(transition: &InstallTransition) -> Value {
    json!({
        "kind": "local_account",
        "accountId": transition.request().account_id(),
    })
}

fn common(
    transition: &InstallTransition,
    kind: &str,
    before: Value,
    after: Value,
    cause: &str,
) -> Value {
    json!({
        "kind": kind,
        "target": {
            "agent": transition.agent().as_str(),
            "artifact": artifact(transition.target()),
        },
        "transition": {"before": before, "after": after},
        "cause": cause,
        "initiator": caller(transition),
        "correlationId": transition.request().request_id(),
    })
}

fn record_value(transition: &InstallTransition) -> Value {
    match transition.kind() {
        InstallTransitionKind::Started => common(
            transition,
            "agent_runtime_install_started",
            json!({"state": "not_started"}),
            json!({"state": "install_started"}),
            "install_requested",
        ),
        InstallTransitionKind::Verified => common(
            transition,
            "agent_runtime_archive_verified",
            json!({"state": "install_started"}),
            json!({"state": "archive_verified"}),
            "pinned_digest_matched",
        ),
        InstallTransitionKind::DigestRejected => {
            let mut value = common(
                transition,
                "agent_runtime_archive_rejected",
                json!({"state": "install_started"}),
                json!({"state": "archive_rejected"}),
                "pinned_digest_mismatched",
            );
            value["actualDigest"] = json!(transition
                .actual_digest()
                .expect("rejection carries actual digest")
                .as_str());
            value
        }
        InstallTransitionKind::Installed => common(
            transition,
            "agent_runtime_installed",
            json!({"state": "archive_verified"}),
            installed(transition.target()),
            "install_requested",
        ),
        InstallTransitionKind::Replaced => common(
            transition,
            "agent_runtime_replaced",
            installed(
                transition
                    .previous()
                    .expect("replacement carries previous artifact"),
            ),
            installed(transition.target()),
            "install_requested",
        ),
        InstallTransitionKind::RolledBack => {
            let after = match transition
                .rollback()
                .expect("rollback carries restored state")
            {
                RollbackState::Restored(artifact) => installed(artifact),
                RollbackState::NoInstalledRuntime => json!({"state": "not_installed"}),
            };
            common(
                transition,
                "agent_runtime_install_rolled_back",
                json!({"state": "publication_pending", "artifact": artifact(transition.target())}),
                after,
                "publication_failed",
            )
        }
        InstallTransitionKind::RecoveryIncomplete => {
            let (state, failures) = transition
                .recovery()
                .expect("incomplete recovery carries state and failure evidence");
            let after = match state {
                RecoveryState::Confirmed(RollbackState::Restored(artifact)) => installed(artifact),
                RecoveryState::Confirmed(RollbackState::NoInstalledRuntime) => {
                    json!({"state": "not_installed"})
                }
                RecoveryState::Unconfirmed => json!({"state": "unconfirmed"}),
            };
            let mut value = common(
                transition,
                "agent_runtime_install_recovery_incomplete",
                json!({"state": "publication_pending", "artifact": artifact(transition.target())}),
                after,
                "publication_cleanup_failed",
            );
            value["failures"] = json!({
                "publication": failure(failures.publication()),
                "withdrawal": failures.withdrawal().map(failure),
                "restoration": failures.restoration().map(failure),
                "confirmation": failures.confirmation().map(failure),
            });
            value
        }
        InstallTransitionKind::SupersededArtifactRemoved => {
            let mut value = common(
                transition,
                "agent_runtime_superseded_artifact_removed",
                installed(transition.target()),
                json!({"state": "removed"}),
                "superseded_by_install",
            );
            value["initiator"] = json!({"kind": "automatic"});
            value["triggeredBy"] = caller(transition);
            value["current"] = artifact(
                transition
                    .current()
                    .expect("removal carries current artifact"),
            );
            value
        }
    }
}

fn audit_failure(error: impl std::fmt::Display) -> AuditFailure {
    AuditFailure(error.to_string())
}

#[cfg(test)]
#[path = "../../../tests/agent_install/audit.rs"]
mod tests;
