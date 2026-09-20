//! Durable attachment evidence: one private file per record, synced with its
//! directory before the record is acknowledged. Separate files cannot tear
//! each other, and concurrent records do not wait on one another.
//!
//! The record's identity and the time it was observed are assigned here, from
//! the injected clock. `requestedAtMs` is when the request that caused the
//! transition was made (a ticket's issue, a release's arrival); it is never
//! presented as the time of the transition itself. Tickets live in memory, so
//! a ticket outstanding when the process stops leaves no record.
use crate::attachments::{
    application::{
        AttachmentAudit, AttachmentAuditRecord, AuditUnavailable, PortFuture, ReleaseCause,
        ReleaseEvidence, UploadRejection,
    },
    domain::{Attachment, Caller, Hold, HoldState, UploadTicket},
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, sync::Arc};
use uuid::Uuid;

/// Private host audit storage for attachment transitions.
pub struct DurableAttachmentAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}
impl DurableAttachmentAudit {
    /// Use a private `directory`. An existing unsafe path fails here; it is
    /// never repaired.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, AuditUnavailable> {
        create_directory(&directory).map_err(|_| AuditUnavailable)?;
        Ok(Self { directory, clock })
    }
}
impl AttachmentAudit for DurableAttachmentAudit {
    fn record(&self, record: AttachmentAuditRecord) -> PortFuture<'_, (), AuditUnavailable> {
        let id = Uuid::new_v4().to_string();
        let mut value = record_value(&record);
        value["recordId"] = json!(id);
        value["observedAtMs"] = json!(self.clock.unix_milliseconds());
        let directory = self.directory.clone();
        Box::pin(async move {
            // The blocking task owns the write even if its caller stops waiting.
            tokio::task::spawn_blocking(move || {
                let mut file = PrivateTempFile::new_in(&directory)?;
                serde_json::to_writer(file.as_file_mut(), &value)?;
                file.as_file_mut().write_all(b"\n")?;
                file.as_file().sync_all()?;
                file.persist(&directory.join(format!("{id}.json")))?;
                sync_directory(&directory)
            })
            .await
            .map_err(|_| AuditUnavailable)?
            .map_err(|error| {
                tracing::error!(%error, "attachment audit record was not committed");
                AuditUnavailable
            })
        })
    }
}

fn hold_state(state: HoldState) -> &'static str {
    match state {
        HoldState::Absent => "absent",
        HoldState::Held => "held",
    }
}

fn rejection(reason: UploadRejection) -> &'static str {
    match reason {
        UploadRejection::SizeMismatch => "size_mismatch",
        UploadRejection::DigestMismatch => "digest_mismatch",
        UploadRejection::BodyInterrupted => "body_interrupted",
        UploadRejection::DeadlineElapsed => "deadline_elapsed",
        UploadRejection::StorageUnavailable => "storage_unavailable",
        UploadRejection::UnsupportedImage => "unsupported_image",
        UploadRejection::ImageTooLarge => "image_too_large",
        UploadRejection::NormalizationFailed => "normalization_failed",
    }
}

fn release_cause(cause: ReleaseCause) -> &'static str {
    match cause {
        ReleaseCause::ConversationClosed => "conversation_closed",
    }
}

fn file(attachment: &Attachment) -> Value {
    json!({
        "digest": attachment.digest().to_string(),
        "mediaType": attachment.media_type().as_str(),
        "size": attachment.size(),
    })
}

fn caller(caller: &Caller) -> Value {
    json!({
        "kind": "caller",
        "principalId": caller.principal_id().as_str(),
        "surfaceId": caller.surface_id(),
    })
}

fn hold_target(hold: &Hold) -> Value {
    json!({
        "organizationId": hold.organization_id().as_str(),
        "conversationId": hold.conversation_id().to_string(),
        "uploaded": file(hold.uploaded()),
        "stored": file(hold.stored()),
    })
}

/// A ticket never reached storage, so its target has no stored file.
fn ticket_target(ticket: &UploadTicket) -> Value {
    json!({
        "organizationId": ticket.organization_id().as_str(),
        "conversationId": ticket.conversation_id().to_string(),
        "uploaded": file(ticket.attachment()),
        "stored": Value::Null,
    })
}

fn released(
    kind: &str,
    before: &str,
    cause: &str,
    hold: &Hold,
    release: &ReleaseEvidence,
) -> Value {
    json!({
        "kind": kind,
        "target": hold_target(hold),
        "transition": {"before": before, "after": "absent"},
        "cause": cause,
        "initiator": caller(&release.caller),
        "correlationId": release.caller.action_id(),
        "requestedAtMs": release.requested_at_ms,
    })
}

/// Everything except the record's identity and observation time.
pub(super) fn record_value(record: &AttachmentAuditRecord) -> Value {
    match record {
        AttachmentAuditRecord::HoldCreated { hold, before } => json!({
            "kind": "attachment_hold_created",
            "target": hold_target(hold),
            "transition": {"before": hold_state(*before), "after": hold_state(HoldState::Held)},
            "cause": "uploaded",
            "initiator": caller(hold.uploaded_by()),
            "correlationId": hold.uploaded_by().action_id(),
            "requestedAtMs": hold.ticket_issued_at_ms(),
            "uploadedAtMs": hold.uploaded_at_ms(),
        }),
        AttachmentAuditRecord::UploadRejected { ticket, reason } => json!({
            "kind": "attachment_upload_rejected",
            "target": ticket_target(ticket),
            "transition": {"before": "ticket_outstanding", "after": "ticket_used_without_hold"},
            "cause": rejection(*reason),
            "initiator": caller(ticket.caller()),
            "correlationId": ticket.caller().action_id(),
            "requestedAtMs": ticket.lifetime().issued_at_ms(),
        }),
        // Nobody expires a ticket. The caller it was issued to is part of what
        // expired, not the initiator of its expiry.
        AttachmentAuditRecord::TicketExpired { ticket } => json!({
            "kind": "attachment_ticket_expired",
            "target": ticket_target(ticket),
            "transition": {"before": "ticket_outstanding", "after": "ticket_expired"},
            "cause": "ticket_lifetime_elapsed",
            "initiator": {"kind": "automatic"},
            "issuedTo": caller(ticket.caller()),
            "correlationId": ticket.caller().action_id(),
            "requestedAtMs": ticket.lifetime().issued_at_ms(),
            "expiredAtMs": ticket.lifetime().expires_at_ms(),
        }),
        AttachmentAuditRecord::TicketVoided { ticket, release } => json!({
            "kind": "attachment_ticket_voided",
            "target": ticket_target(ticket),
            "transition": {"before": "ticket_outstanding", "after": "ticket_voided"},
            "cause": release_cause(release.cause),
            "initiator": caller(&release.caller),
            "issuedTo": caller(ticket.caller()),
            "correlationId": release.caller.action_id(),
            "ticketCorrelationId": ticket.caller().action_id(),
            "requestedAtMs": release.requested_at_ms,
        }),
        AttachmentAuditRecord::HoldReleased { hold, release } => released(
            "attachment_hold_released",
            hold_state(HoldState::Held),
            release_cause(release.cause),
            hold,
            release,
        ),
        // The caller's release is why the last hold went; that the bytes then
        // had no holder is why they were removed.
        AttachmentAuditRecord::BlobRemoved { hold, release } => {
            let mut value = released(
                "attachment_bytes_removed",
                "stored",
                "last_hold_released",
                hold,
                release,
            );
            value["releaseCause"] = json!(release_cause(release.cause));
            value
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/attachments/audit.rs"]
mod tests;
