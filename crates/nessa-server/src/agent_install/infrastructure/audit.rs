//! Durable install evidence: one private file per transition, synced before
//! acknowledgement. `observedAtMs` is when this adapter received the record;
//! effect ordering comes from each transition's before/after chain, not clocks.
use std::{io::Write, path::PathBuf, sync::Arc};

use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent_install::{
    application::{AuditFailure, InstallAudit},
    domain::{InstallTransition, InstallTransitionKind, RollbackState, RuntimeArtifact},
};

/// Private host audit storage for agent-runtime installation transitions.
pub struct DurableInstallAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableInstallAudit {
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, AuditFailure> {
        create_directory(&directory).map_err(audit_failure)?;
        Ok(Self { directory, clock })
    }
}

impl InstallAudit for DurableInstallAudit {
    fn record(&self, transition: InstallTransition) -> Result<(), AuditFailure> {
        let id = Uuid::new_v4().to_string();
        let mut value = record_value(&transition);
        value["recordId"] = json!(id);
        value["observedAtMs"] = json!(self.clock.unix_milliseconds());
        let mut file = PrivateTempFile::new_in(&self.directory).map_err(audit_failure)?;
        serde_json::to_writer(file.as_file_mut(), &value)
            .map_err(|error| AuditFailure(error.to_string()))?;
        file.as_file_mut().write_all(b"\n").map_err(audit_failure)?;
        file.as_file().sync_all().map_err(audit_failure)?;
        file.persist(&self.directory.join(format!("{id}.json")))
            .map_err(audit_failure)?;
        sync_directory(&self.directory).map_err(audit_failure)
    }
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
