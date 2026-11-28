//! Durable evidence of each app install and uninstall, in
//! `<namespace>/conversations/audit/apps`: one private JSON file per record,
//! named by its operation and phase, published once and synced, as the voice
//! audit beside it is. The package's digest names what was installed; its
//! bytes are never copied here.
use std::{io::Write, path::PathBuf, sync::Arc};

use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};

use crate::installed_apps::application::{AppsAudit, AppsEvent, AppsRecord, AuditUnavailable};

pub struct DurableAppsAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableAppsAudit {
    /// The audit in `directory`, created private by its first record.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self { directory, clock }
    }
}

/// The record's phase in its file name, and what it holds.
fn stored(record: &AppsRecord) -> (&'static str, Value) {
    let (phase, kind, details) = match &record.event {
        AppsEvent::InstallRequested {
            digest,
            package_version,
        } => (
            "requested",
            "app_install",
            json!({ "digest": digest, "packageVersion": package_version, "before": "absent" }),
        ),
        AppsEvent::Installed { digest } => (
            "installed",
            "app_install",
            json!({ "digest": digest, "after": "installed" }),
        ),
        AppsEvent::InstallFailed { reason } => (
            "failed",
            "app_install",
            json!({ "reason": reason, "after": "absent" }),
        ),
        AppsEvent::UninstallRequested { package_version } => (
            "requested",
            "app_uninstall",
            json!({ "packageVersion": package_version, "before": "installed" }),
        ),
        AppsEvent::Uninstalled => ("uninstalled", "app_uninstall", json!({ "after": "absent" })),
        AppsEvent::UninstallFailed { reason } => (
            "failed",
            "app_uninstall",
            json!({ "reason": reason, "after": "installed" }),
        ),
    };
    (
        phase,
        json!({
            "kind": kind,
            "operationId": record.operation_id,
            "requestId": record.request_id,
            "appId": record.app_id,
            "phase": phase,
            "details": details,
            "cause": "caller_requested",
            "initiator": {
                "kind": "caller",
                "organizationId": record.caller.organization_id,
                "principalId": record.caller.principal_id,
                "credentialId": record.caller.credential_id,
            },
        }),
    )
}

impl AppsAudit for DurableAppsAudit {
    fn record(&self, record: &AppsRecord) -> Result<(), AuditUnavailable> {
        let (phase, mut value) = stored(record);
        create_directory(&self.directory).map_err(|_| AuditUnavailable)?;
        let id = format!("app-{}-{phase}", record.operation_id);
        if let Value::Object(fields) = &mut value {
            fields.insert("recordId".into(), json!(id));
            fields.insert("observedAtMs".into(), json!(self.clock.unix_milliseconds()));
        }
        let mut file = PrivateTempFile::new_in(&self.directory).map_err(|_| AuditUnavailable)?;
        serde_json::to_writer(file.as_file_mut(), &value).map_err(|_| AuditUnavailable)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| AuditUnavailable)?;
        // One operation, one record of each phase: a name taken is a fault.
        file.publish(&self.directory.join(format!("{id}.json")))
            .map_err(|_| AuditUnavailable)?;
        sync_directory(&self.directory).map_err(|_| AuditUnavailable)
    }
}
