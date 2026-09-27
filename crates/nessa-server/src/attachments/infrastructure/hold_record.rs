//! The saved form of a hold. Domain values are mapped field by field, and a
//! saved record becomes a hold again only through the domain's constructors.
//!
//! Beside the hold, a record says whether it is usable yet and which write
//! produced it. A `pending` record is a hold whose evidence had not been
//! committed: nothing that asks what a conversation holds may see it. The
//! `generation` is what a claim on the record is compared with.
use crate::{
    attachments::domain::{Attachment, Caller, Hold, MediaType},
    conversation::domain::ConversationId,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use serde::{Deserialize, Serialize};

/// Whether a saved hold may be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordState {
    Pending,
    Kept,
}

/// A saved hold, with what the store alone knows about it.
pub(super) struct HoldRecord {
    pub hold: Hold,
    pub state: RecordState,
    pub generation: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredAttachment {
    digest: String,
    media_type: String,
    size: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredCaller {
    principal_id: String,
    surface_id: String,
    action_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredHold {
    state: String,
    generation: String,
    organization_id: String,
    conversation_id: String,
    uploaded: StoredAttachment,
    stored: StoredAttachment,
    uploaded_by: StoredCaller,
    ticket_issued_at_ms: u64,
    uploaded_at_ms: u64,
}

fn stored_attachment(attachment: &Attachment) -> StoredAttachment {
    StoredAttachment {
        digest: attachment.digest().to_string(),
        media_type: attachment.media_type().as_str().to_owned(),
        size: attachment.size(),
    }
}

fn attachment(stored: &StoredAttachment) -> Option<Attachment> {
    Attachment::new(
        Sha256Digest::parse(&stored.digest).ok()?,
        MediaType::parse(&stored.media_type).ok()?,
        stored.size,
    )
    .ok()
}

fn state_name(state: RecordState) -> &'static str {
    match state {
        RecordState::Pending => "pending",
        RecordState::Kept => "kept",
    }
}

pub(super) fn encode(hold: &Hold, state: RecordState, generation: &str) -> Vec<u8> {
    let caller = hold.uploaded_by();
    serde_json::to_vec(&StoredHold {
        state: state_name(state).to_owned(),
        generation: generation.to_owned(),
        organization_id: hold.organization_id().as_str().to_owned(),
        conversation_id: hold.conversation_id().to_string(),
        uploaded: stored_attachment(hold.uploaded()),
        stored: stored_attachment(hold.stored()),
        uploaded_by: StoredCaller {
            principal_id: caller.principal_id().as_str().to_owned(),
            surface_id: caller.surface_id().to_owned(),
            action_id: caller.action_id().to_owned(),
        },
        ticket_issued_at_ms: hold.ticket_issued_at_ms(),
        uploaded_at_ms: hold.uploaded_at_ms(),
    })
    .expect("a hold record is plain strings and numbers")
}

/// `None` for anything that is not a complete, possible hold.
pub(super) fn decode(bytes: &[u8]) -> Option<HoldRecord> {
    let record: StoredHold = serde_json::from_slice(bytes).ok()?;
    let state = match record.state.as_str() {
        "pending" => RecordState::Pending,
        "kept" => RecordState::Kept,
        _ => return None,
    };
    if record.generation.is_empty() || record.generation.len() > 64 {
        return None;
    }
    let hold = Hold::restore(
        OrganizationId::new(record.organization_id).ok()?,
        ConversationId::new(&record.conversation_id).ok()?,
        attachment(&record.uploaded)?,
        attachment(&record.stored)?,
        Caller::new(
            PrincipalId::new(record.uploaded_by.principal_id).ok()?,
            &record.uploaded_by.surface_id,
            &record.uploaded_by.action_id,
        )
        .ok()?,
        record.ticket_issued_at_ms,
        record.uploaded_at_ms,
    )?;
    Some(HoldRecord {
        hold,
        state,
        generation: record.generation,
    })
}
