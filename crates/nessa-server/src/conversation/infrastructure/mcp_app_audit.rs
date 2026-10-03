//! Durable evidence of every step of an MCP App's call (#348), committed before
//! the step's effect is reported.
//!
//! ```text
//!   service ──record──▶ this adapter ──▶ <root>/audit/mcp-apps/<name>.json
//!      │                                          │
//!      └── refuses the step on failure            └── one file per step, private,
//!                                                     named from a digest of the
//!                                                     step's identity
//! ```
//!
//! Arrows are calls and writes. A step's identity is its conversation, its app
//! (the tool call whose UI it is, and which mount of it acted), the call id
//! the gateway minted for this one call, and the kind of phase. Two phases of
//! one call are two records; the same phase recorded again —
//! a retry — reconciles with the record already there, and anything else under
//! that identity is contradictory evidence and fails closed.
//!
//! What is stored is who, what was asked, and what happened: never the call's
//! arguments, its result, or a resource's bytes. A resource ticket appears only
//! as the digest the port already carries, and a result only as its size.
//!
//! Durability: the record is written to a private temporary file, synced,
//! published create-only under its name, and the directory is synced before
//! the step is reported as recorded. `observedAtMs` is when this adapter saw
//! the step, from the injected clock — not the unknown time of the server's
//! effect — and the first writer's observation is the one kept.
use crate::conversation::application::{
    ConversationError, ConversationFuture, McpAppAsk, McpAppAudit, McpAppAuditPhase,
    McpAppAuditRecord, McpAppInitiator, McpAppOutcome, McpAppWithdrawal, TicketEnd,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Most bytes one record may be. The port's strings are not bounded by type,
/// so a record larger than this is refused when written rather than stored as
/// something no later retry could read back and agree with.
const MAX_RECORD_BYTES: usize = 65_536;

/// The one field two writers of the same step may honestly differ on: when
/// each happened to see it. Everything else is the step itself.
const MAY_DIFFER_BETWEEN_WRITERS: &str = "observedAtMs";

pub struct DurableMcpAppAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableMcpAppAudit {
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}

impl McpAppAudit for DurableMcpAppAudit {
    fn record(&self, record: McpAppAuditRecord) -> ConversationFuture<'_, ()> {
        let phase = phase(&record.phase);
        let kind = phase["kind"].clone();
        // Every part of the identity is caller text except the conversation,
        // so the name is a digest of all of it, encoded as a JSON array so no
        // two different identities can spell the same input.
        let identity = json!([
            record.conversation_id.to_string(),
            record.app.execution_id,
            record.app.tool_id,
            record.app.instance_id,
            record.call_id,
            kind,
        ]);
        let id = format!("mcp-app-{}", digest_of(identity.to_string().as_bytes()));
        let value = json!({
            "recordId": id,
            "kind": "mcp_app_call",
            "target": {
                "conversationId": record.conversation_id.to_string(),
                "organizationId": record.organization_id.as_str(),
                "app": {
                    "executionId": record.app.execution_id,
                    "toolId": record.app.tool_id,
                    "instanceId": record.app.instance_id,
                },
                "ask": ask(&record.ask),
            },
            "callId": record.call_id,
            "requestId": record.request_id,
            "phase": phase,
            "initiator": initiator(&record.initiator),
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || write(&directory, &id, &value))
                .await
                .map_err(|_| ConversationError::Audit)?
        })
    }
}

/// Write `value` as the record `id`, or agree with the one already there.
fn write(directory: &Path, id: &str, value: &Value) -> Result<(), ConversationError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ConversationError::Audit)?;
    if bytes.len() + 1 > MAX_RECORD_BYTES {
        tracing::error!(record = %id, "an MCP App record is larger than any this stores");
        return Err(ConversationError::Audit);
    }
    let destination = directory.join(format!("{id}.json"));
    // Reading first gives the better failure for a contradiction. It is not
    // the exclusion: two writers can both read nothing, and the create-only
    // publish below settles which one wrote.
    if let Stored::Agrees = stored_evidence(&destination, value, id)? {
        return sync_directory(directory).map_err(audit("sync a directory"));
    }
    let mut file = PrivateTempFile::new_in(directory).map_err(audit("open a record"))?;
    file.as_file_mut()
        .write_all(&bytes)
        .and_then(|_| file.as_file_mut().write_all(b"\n"))
        .and_then(|_| file.as_file().sync_all())
        .map_err(audit("write a record"))?;
    if let Err(error) = file.publish(&destination) {
        if error.kind() != ErrorKind::AlreadyExists {
            tracing::error!(record = %id, %error, "could not publish an MCP App record");
            return Err(ConversationError::Audit);
        }
        // A writer that lost the race is held to what won it. A name taken
        // and then gone is not agreement.
        return match stored_evidence(&destination, value, id)? {
            Stored::Agrees => sync_directory(directory).map_err(audit("sync a directory")),
            Stored::Absent => {
                tracing::error!(record = %id, "an MCP App record published first is gone");
                Err(ConversationError::Audit)
            }
        };
    }
    sync_directory(directory).map_err(audit("sync a directory"))
}

fn audit(doing: &'static str) -> impl Fn(io::Error) -> ConversationError {
    move |error| {
        tracing::error!(%error, "could not {doing} for MCP App evidence");
        ConversationError::Audit
    }
}

/// What is already stored for this step.
enum Stored {
    Absent,
    Agrees,
}

/// Read what is stored at `destination` and hold it to `value`.
///
/// # Errors
///
/// [`ConversationError::Audit`] when the stored record cannot be read, is
/// larger than any this writes, is not JSON, or disagrees with `value` about
/// anything but [`MAY_DIFFER_BETWEEN_WRITERS`].
fn stored_evidence(
    destination: &Path,
    value: &Value,
    id: &str,
) -> Result<Stored, ConversationError> {
    let mut file = match open(destination, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Stored::Absent),
        Err(error) => {
            tracing::error!(record = %id, %error, "could not read stored MCP App evidence");
            return Err(ConversationError::Audit);
        }
    };
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(audit("read a record"))?;
    if bytes.len() > MAX_RECORD_BYTES {
        tracing::error!(record = %id, "a stored MCP App record is larger than any this writes");
        return Err(ConversationError::Audit);
    }
    let stored: Value = serde_json::from_slice(&bytes).map_err(|_| ConversationError::Audit)?;
    if comparable(stored) != comparable(value.clone()) {
        tracing::error!(record = %id, "stored MCP App evidence contradicts this step");
        return Err(ConversationError::Audit);
    }
    Ok(Stored::Agrees)
}

fn comparable(mut value: Value) -> Value {
    let _ = value
        .as_object_mut()
        .and_then(|object| object.remove(MAY_DIFFER_BETWEEN_WRITERS));
    value
}

fn digest_of(bytes: &[u8]) -> String {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into()).to_hex()
}

fn ask(ask: &McpAppAsk) -> Value {
    match ask {
        McpAppAsk::CallTool { server, tool } => {
            json!({"kind": "call_tool", "server": server, "tool": tool})
        }
        McpAppAsk::ReadResource { server, uri } => {
            json!({"kind": "read_resource", "server": server, "uri": uri})
        }
        McpAppAsk::SendMessage { server } => json!({"kind": "send_message", "server": server}),
        McpAppAsk::UpdateModelContext { server } => {
            json!({"kind": "update_model_context", "server": server})
        }
    }
}

fn initiator(initiator: &McpAppInitiator) -> Value {
    match initiator {
        McpAppInitiator::App {
            principal_id,
            surface_id,
        } => json!({
            "kind": "app",
            "onBehalfOf": {"principalId": principal_id.as_str(), "surfaceId": surface_id},
        }),
        McpAppInitiator::Person {
            principal_id,
            surface_id,
            request_id,
        } => json!({
            "kind": "person",
            "principalId": principal_id.as_str(),
            "surfaceId": surface_id,
            "requestId": request_id,
        }),
        McpAppInitiator::System => json!({"kind": "system"}),
    }
}

/// The phase, with `kind` naming it. `kind` is also part of the record's
/// identity, so each arm's name must be distinct;
/// `every_phase_of_one_request_is_its_own_record` holds them to it.
fn phase(phase: &McpAppAuditPhase) -> Value {
    match phase {
        McpAppAuditPhase::Refused(code) => json!({"kind": "refused", "code": code.as_str()}),
        McpAppAuditPhase::Admitted => json!({"kind": "admitted"}),
        McpAppAuditPhase::ApprovalRequested { permission_id } => {
            json!({"kind": "approval_requested", "permissionId": permission_id})
        }
        McpAppAuditPhase::Approved { permission_id } => {
            json!({"kind": "approved", "permissionId": permission_id})
        }
        McpAppAuditPhase::Denied { permission_id } => {
            json!({"kind": "denied", "permissionId": permission_id})
        }
        McpAppAuditPhase::Expired { permission_id } => {
            json!({"kind": "expired", "permissionId": permission_id})
        }
        McpAppAuditPhase::Withdrawn {
            permission_id,
            cause,
        } => json!({
            "kind": "withdrawn",
            "permissionId": permission_id,
            "cause": withdrawal(*cause),
        }),
        McpAppAuditPhase::Completed(outcome) => json!({
            "kind": "completed",
            "outcome": match outcome {
                McpAppOutcome::Answered { is_error, bytes } => {
                    json!({"kind": "answered", "isError": is_error, "bytes": bytes})
                }
                McpAppOutcome::Failed(code) => json!({"kind": "failed", "code": code.as_str()}),
            },
        }),
        McpAppAuditPhase::TicketIssued {
            ticket_digest,
            size,
            sha256,
        } => json!({
            "kind": "ticket_issued",
            "ticketDigest": ticket_digest,
            "size": size,
            "sha256": sha256,
        }),
        McpAppAuditPhase::TicketRedeemed { ticket_digest } => {
            json!({"kind": "ticket_redeemed", "ticketDigest": ticket_digest})
        }
        McpAppAuditPhase::TicketEnded {
            ticket_digest,
            cause,
        } => json!({
            "kind": "ticket_ended",
            "ticketDigest": ticket_digest,
            "cause": ticket_end(*cause),
        }),
        McpAppAuditPhase::MessageSent { execution_id } => {
            json!({"kind": "message_sent", "executionId": execution_id})
        }
        McpAppAuditPhase::MessageNotSent { execution_id } => {
            json!({"kind": "message_not_sent", "executionId": execution_id})
        }
        McpAppAuditPhase::ContextHeld { bytes } => json!({"kind": "context_held", "bytes": bytes}),
        McpAppAuditPhase::ContextCleared => json!({"kind": "context_cleared"}),
    }
}

fn ticket_end(cause: TicketEnd) -> &'static str {
    match cause {
        TicketEnd::Expired => "expired",
        TicketEnd::AppReleased => "app_released",
        TicketEnd::ConversationEnded => "conversation_ended",
    }
}

fn withdrawal(cause: McpAppWithdrawal) -> &'static str {
    match cause {
        McpAppWithdrawal::RequestCancelled => "request_cancelled",
        McpAppWithdrawal::AppTornDown => "app_torn_down",
        McpAppWithdrawal::ConversationEnded => "conversation_ended",
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/mcp_app_audit.rs"]
mod tests;
