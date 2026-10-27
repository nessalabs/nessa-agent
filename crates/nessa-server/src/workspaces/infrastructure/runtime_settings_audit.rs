//! Private requested/outcome records for ordinary runtime configuration edits.
//! The same operation ID joins intent to publication, including uncertain writes.
use crate::conversation::domain::{VmLifetime, VmLifetimeScope};
use crate::workspaces::{
    application::{RuntimeAudit, RuntimeAuditEvent, RuntimePublication},
    domain::Failure,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, sync::Arc};

pub(crate) struct RuntimeSettingsAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}
impl RuntimeSettingsAudit {
    pub(crate) fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, Failure> {
        create_directory(&directory).map_err(|_| Failure::Storage)?;
        Ok(Self { directory, clock })
    }
}
fn choice(lifetime: Option<VmLifetime>) -> Value {
    match lifetime {
        None => json!({"mode":"default"}),
        Some(VmLifetime::Unlimited) => json!({"mode":"never"}),
        Some(lifetime) => json!({"mode":"after","seconds":lifetime.seconds()}),
    }
}
impl RuntimeAudit for RuntimeSettingsAudit {
    fn record(&self, event: &RuntimeAuditEvent) -> Result<(), Failure> {
        let (change, phase, outcome) = match event {
            RuntimeAuditEvent::Requested(change) => (change, "requested", None),
            RuntimeAuditEvent::Outcome(change, outcome) => (
                change,
                "outcome",
                Some(match outcome {
                    RuntimePublication::Unchanged => "unchanged",
                    RuntimePublication::Durable => "durable",
                    RuntimePublication::NotDurable => "not_durable",
                    RuntimePublication::Failed => "failed",
                    RuntimePublication::Uncertain => "uncertain",
                }),
            ),
        };
        let after = match event {
            RuntimeAuditEvent::Requested(_)
            | RuntimeAuditEvent::Outcome(_, RuntimePublication::Uncertain) => None,
            RuntimeAuditEvent::Outcome(
                _,
                RuntimePublication::Durable | RuntimePublication::NotDurable,
            ) => Some(choice(change.requested)),
            RuntimeAuditEvent::Outcome(
                _,
                RuntimePublication::Unchanged | RuntimePublication::Failed,
            ) => Some(choice(change.before)),
        };
        let target = match &change.scope {
            VmLifetimeScope::Global => json!({"scope":"global"}),
            VmLifetimeScope::Channel(channel) => {
                json!({"scope":"channel","channelId":channel.as_str()})
            }
        };
        let record = json!({
            "kind":"workspace_runtime_settings", "operationId":change.id.to_string(),
            "phase":phase, "outcome":outcome, "observedAtMs":self.clock.unix_milliseconds(),
            "target":target, "before":choice(change.before), "requested":choice(change.requested), "after":after,
            "cause":"caller_requested", "initiator":{
                "kind":"caller", "organizationId":change.caller.organization.as_str(),
                "principalId":change.caller.principal.as_str(), "requestId":change.caller.operation,
                "credentialId":change.caller.credential,
            }
        });
        let mut file = PrivateTempFile::new_in(&self.directory).map_err(|_| Failure::Storage)?;
        serde_json::to_writer(file.as_file_mut(), &record).map_err(|_| Failure::Storage)?;
        file.as_file_mut()
            .write_all(b"\n")
            .map_err(|_| Failure::Storage)?;
        file.as_file_mut()
            .sync_all()
            .map_err(|_| Failure::Storage)?;
        file.publish(&self.directory.join(format!("{}-{phase}.json", change.id)))
            .map_err(|_| Failure::Storage)?;
        sync_directory(&self.directory).map_err(|_| Failure::Storage)
    }
}
