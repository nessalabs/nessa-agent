//! Durable evidence of each change to the stored MCP servers, and of each
//! inspection (a stored server's executable run once), in
//! `<namespace>/conversations/audit/mcp-servers`: one private JSON file per
//! record, named by its change and phase, published once and synced, as the
//! other audits beside it are. A record names servers and variables, never a
//! variable's value
//! (`composed_settings_publish_privately_under_the_lock_and_audit_without_values`).
//!
//! Records carry no sequence number. An operation's requested record is
//! made durable before its outcome; between operations — and across a
//! restart — the only order is `observedAtMs`, on the wall clock, which may
//! step backwards. The operation's id, not the time, is what pairs a
//! requested record with its outcome.
use crate::mcp_servers::application::{
    AuditUnavailable, AuditedServer, InspectCut, LiveSetOutcome, McpServerAction, McpServerAudit,
    McpServerAuditPhase, McpServerAuditRecord, McpServerCause, McpServerOutcome, ServerNames,
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

/// A server as a record names it: what it is started with and its
/// variables' names, never their values.
fn server(server: &AuditedServer) -> Value {
    let mut value = json!({
        "name": server.name,
        "command": server.command.to_string_lossy(),
        "args": server.args,
        "enabled": server.enabled,
        "envNames": server.env_names,
    });
    if let Some(url) = &server.url {
        value["url"] = json!(url);
    }
    if let Some(id) = &server.remote_id {
        value["id"] = json!(id);
    }
    value
}

/// A revision, its names, and the change's target there.
fn names(names: &ServerNames) -> Value {
    json!({
        "revision": names.revision,
        "names": names.names,
        "target": names.target.as_deref().map(server),
    })
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
                "server": request.server.as_deref().map(server),
            }),
        ),
        // The server asked for, with its executable and arguments, so a
        // refused save still names them.
        McpServerAuditPhase::Requested => (
            "requested",
            json!({
                "requestedRevision": request.revision,
                "server": request.server.as_deref().map(server),
            }),
        ),
        McpServerAuditPhase::Outcome(McpServerOutcome::Applied {
            before,
            after,
            live_set,
            durable,
        }) => (
            "outcome",
            json!({
                "outcome": "applied",
                "before": names(before),
                "after": names(after),
                "liveSet": match live_set {
                    LiveSetOutcome::Replaced => "replaced",
                    LiveSetOutcome::Withdrawn => "withdrawn",
                    LiveSetOutcome::Kept => "kept",
                },
                "durable": durable,
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
        // Whether the server was started is said either way, so a record of
        // `stopping` says that nothing ran.
        McpServerAuditPhase::Outcome(McpServerOutcome::InspectFailed { reason, started }) => (
            "outcome",
            json!({"outcome": "failed", "reason": reason, "started": started}),
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
                    InspectCut::Stopping => "stopping",
                }),
            }),
        ),
    };
    // The operation's cause, and who set it off: the caller, or the gateway
    // stopping, for an inspection shutdown ended.
    let (cause, initiator) = match &record.cause {
        McpServerCause::CallerRequested(caller) => (
            "caller_requested",
            json!({
                "kind": "caller",
                "organizationId": caller.organization_id,
                "principalId": caller.principal_id,
                "credentialId": caller.credential_id,
            }),
        ),
        McpServerCause::GatewayStopping => ("gateway_stopping", json!({"kind": "system"})),
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
        "cause": cause,
        "initiator": initiator,
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
