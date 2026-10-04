//! Durable evidence of each change to the stored MCP servers, and of each
//! inspection (a stored server's executable run once), in
//! `<namespace>/conversations/audit/mcp-servers`: one private JSON file per
//! record, named by its change and phase, published once and synced, as the
//! other audits beside it are. A record names servers and variables, never a
//! variable's value
//! (`composed_settings_publish_privately_under_the_lock_and_audit_without_values`).
use crate::mcp_servers::application::{
    AuditUnavailable, InspectCut, McpServerAction, McpServerAudit, McpServerAuditPhase,
    McpServerAuditRecord, McpServerOutcome, ServerNames,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, sync::Arc};

/// The records, in their directory, stamped with when each was observed on
/// `clock`.
pub struct DurableMcpServerAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableMcpServerAudit {
    /// The audit in `directory`, created private when it is not there.
    ///
    /// # Errors
    ///
    /// [`AuditUnavailable`] when the directory cannot be created.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, AuditUnavailable> {
        create_directory(&directory).map_err(|_| AuditUnavailable)?;
        Ok(Self { directory, clock })
    }
}

fn names(names: &ServerNames) -> Value {
    json!({"revision": names.revision, "names": names.names})
}

/// `record` as stored, without its identity and observation time.
fn stored(record: &McpServerAuditRecord) -> Value {
    let request = &record.request;
    let (phase, transition) = match &record.phase {
        // An inspection names the revision its server was read at, not one
        // the caller named.
        McpServerAuditPhase::Requested if request.action == McpServerAction::Inspect => (
            "requested",
            json!({
                "revision": request.revision,
                "envNames": request.env_names,
                "enabled": request.enabled,
            }),
        ),
        McpServerAuditPhase::Requested => (
            "requested",
            json!({
                "requestedRevision": request.revision,
                "envNames": request.env_names,
                "enabled": request.enabled,
            }),
        ),
        McpServerAuditPhase::Outcome(McpServerOutcome::Applied {
            before,
            after,
            live_set_replaced,
        }) => (
            "outcome",
            json!({
                "outcome": "applied",
                "before": names(before),
                "after": names(after),
                "liveSetReplaced": live_set_replaced,
            }),
        ),
        McpServerAuditPhase::Outcome(McpServerOutcome::Refused { reason, before }) => (
            "outcome",
            json!({"outcome": "refused", "reason": reason, "before": before.as_ref().map(names)}),
        ),
        McpServerAuditPhase::Outcome(McpServerOutcome::Failed { reason, before }) => (
            "outcome",
            json!({"outcome": "failed", "reason": reason, "before": before.as_ref().map(names)}),
        ),
        McpServerAuditPhase::Outcome(McpServerOutcome::Inspected { tools, cut }) => (
            "outcome",
            json!({
                "outcome": "inspected",
                "tools": tools,
                "cut": cut.map(|cut| match cut {
                    InspectCut::Tools => "tools",
                    InspectCut::Ui => "ui",
                    InspectCut::Bytes => "bytes",
                }),
            }),
        ),
    };
    json!({
        "kind": "mcp_servers",
        "operationId": record.operation_id,
        "phase": phase,
        "action": match request.action {
            McpServerAction::Save => "save",
            McpServerAction::Remove => "remove",
            McpServerAction::Inspect => "inspect",
        },
        "target": {"name": request.target, "previousName": request.previous_name},
        "transition": transition,
        "cause": "caller_requested",
        "initiator": {
            "kind": "caller",
            "organizationId": record.initiator.organization_id,
            "principalId": record.initiator.principal_id,
            "credentialId": record.initiator.credential_id,
        },
    })
}

impl McpServerAudit for DurableMcpServerAudit {
    fn record(&self, record: &McpServerAuditRecord) -> Result<(), AuditUnavailable> {
        let phase = match record.phase {
            McpServerAuditPhase::Requested => "requested",
            McpServerAuditPhase::Outcome(_) => "outcome",
        };
        let id = format!("mcp-servers-{}-{phase}", record.operation_id);
        let mut value = stored(record);
        if let Value::Object(fields) = &mut value {
            fields.insert("recordId".into(), json!(id));
            // When this adapter observed the record, for durable delivery.
            fields.insert("observedAtMs".into(), json!(self.clock.unix_milliseconds()));
        }
        let mut file = PrivateTempFile::new_in(&self.directory).map_err(|_| AuditUnavailable)?;
        serde_json::to_writer(file.as_file_mut(), &value).map_err(|_| AuditUnavailable)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| AuditUnavailable)?;
        // One change, one record of each phase: a name taken is a fault.
        file.publish(&self.directory.join(format!("{id}.json")))
            .map_err(|_| AuditUnavailable)?;
        sync_directory(&self.directory).map_err(|_| AuditUnavailable)
    }
}
