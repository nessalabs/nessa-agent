//! Durable evidence of each saved-login change and each login handed to, or
//! withheld from, a machine, in `<namespace>/conversations/audit/logins`:
//! one private JSON file per record, named by its operation and phase,
//! published once and synced, as the voice audit beside it is. Never a login.
use std::{io::Write, path::PathBuf, sync::Arc};

use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};

use crate::logins::application::{AuditUnavailable, LoginAudit, LoginEvent, LoginRecord, Withheld};

pub struct DurableLoginAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableLoginAudit {
    /// The audit in `directory`, created private by its first record.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self { directory, clock }
    }
}

fn withheld(why: Withheld) -> &'static str {
    match why {
        Withheld::NotAllowed => "not_allowed",
        Withheld::NoneSaved => "none_saved",
        Withheld::Locked => "vault_locked",
        Withheld::Unavailable => "unavailable",
    }
}

/// The record's phase in its file name, and what it holds.
pub(super) fn stored(record: &LoginRecord) -> (&'static str, Value) {
    let (phase, kind, details) = match &record.event {
        LoginEvent::SaveRequested { agent, replacing } => (
            "requested",
            "login_save",
            json!({ "agent": agent.name(), "replacing": replacing }),
        ),
        LoginEvent::SaveFailed { agent } => {
            ("failed", "login_save", json!({ "agent": agent.name() }))
        }
        LoginEvent::Saved { agent } => ("saved", "login_save", json!({ "agent": agent.name() })),
        LoginEvent::RemoveRequested { agent } => (
            "requested",
            "login_remove",
            json!({ "agent": agent.name() }),
        ),
        LoginEvent::RemoveFailed { agent } => {
            ("failed", "login_remove", json!({ "agent": agent.name() }))
        }
        LoginEvent::Removed { agent } => {
            ("removed", "login_remove", json!({ "agent": agent.name() }))
        }
        LoginEvent::SwitchRequested { peer, on } => (
            "requested",
            "login_device_switch",
            json!({ "environment": peer.name(), "on": on }),
        ),
        LoginEvent::SwitchFailed { peer, on } => (
            "failed",
            "login_device_switch",
            json!({ "environment": peer.name(), "on": on }),
        ),
        LoginEvent::Switched { peer, on } => (
            "switched",
            "login_device_switch",
            json!({ "environment": peer.name(), "on": on }),
        ),
        LoginEvent::Sent { environment, agent } => (
            "sent",
            "login_handed",
            json!({ "environment": environment, "agent": agent.name() }),
        ),
        LoginEvent::NotSent {
            environment,
            agent,
            why,
        } => (
            "withheld",
            "login_handed",
            json!({ "environment": environment, "agent": agent.name(), "reason": withheld(*why) }),
        ),
    };
    let (cause, initiator) = match &record.caller {
        Some(caller) => (
            "caller_requested",
            json!({
                "kind": "caller",
                "organizationId": caller.organization_id,
                "principalId": caller.principal_id,
                "credentialId": caller.credential_id,
            }),
        ),
        // A login handed out at a lease: the lease's own records name who
        // started it.
        None => ("lease_granted", json!({ "kind": "gateway" })),
    };
    (
        phase,
        json!({
            "kind": kind,
            "operationId": record.operation_id,
            "phase": phase,
            "details": details,
            "cause": cause,
            "initiator": initiator,
        }),
    )
}

impl LoginAudit for DurableLoginAudit {
    fn record(&self, record: &LoginRecord) -> Result<(), AuditUnavailable> {
        let (phase, mut value) = stored(record);
        create_directory(&self.directory).map_err(|_| AuditUnavailable)?;
        let id = format!("login-{}-{phase}", record.operation_id);
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
