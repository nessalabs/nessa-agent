//! The saved form of a hold. Domain values are mapped field by field, and a
//! saved record becomes a hold again only through the domain's constructors.
//!
//! Beside the hold, a record says whether it is usable yet and which write
//! produced it. A `pending` record is a hold whose evidence had not been
//! committed: nothing that asks what a conversation holds may see it. The
//! `generation` is what a claim on the record is compared with. Retired records
//! retain the exact lifetime and actual release or upload-reversal evidence.
//! Prior state excludes Absent by construction through RetiredFrom.
use crate::{
    attachments::{
        application::{
            ReleaseCause, ReleaseEvidence, RetiredHold, RetirementEvidence, RevertCause,
        },
        domain::{Attachment, Caller, Hold, MediaType, RetiredFrom},
    },
    conversation::domain::ConversationId,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Whether a saved hold may be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RecordState {
    Pending,
    Kept,
    Retired {
        was: RetiredFrom,
        evidence: RetirementEvidence,
    },
}

// Saved spelling belongs to this codec; the domain supplies the legal states.
impl Serialize for RetiredFrom {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::Pending => "pending",
            Self::Held => "kept",
        })
    }
}
impl<'de> Deserialize<'de> for RetiredFrom {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "pending" => Ok(Self::Pending),
            "kept" => Ok(Self::Held),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["pending", "kept"],
            )),
        }
    }
}

/// A saved hold, with what the store alone knows about it.
#[derive(Debug, PartialEq, Eq)]
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
    state: StoredState,
    generation: String,
    organization_id: String,
    conversation_id: String,
    uploaded: StoredAttachment,
    stored: StoredAttachment,
    uploaded_by: StoredCaller,
    ticket_issued_at_ms: u64,
    uploaded_at_ms: u64,
    #[serde(default, skip_serializing_if = "SavedField::is_missing")]
    was: SavedField<RetiredFrom>,
    #[serde(default, skip_serializing_if = "SavedField::is_missing")]
    retirement: SavedField<StoredRetirement>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredState {
    Pending,
    Kept,
    Retired,
}

// Missing and present are different: explicit null must not become a missing
// retirement field on an active record. Typed deserialization also rejects
// duplicate and unknown keys, including inside the evidence.
struct SavedField<T>(Option<T>);
impl<T> Default for SavedField<T> {
    fn default() -> Self {
        Self(None)
    }
}
impl<T> SavedField<T> {
    fn is_missing(&self) -> bool {
        self.0.is_none()
    }
}
impl<T: Serialize> Serialize for SavedField<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Some(value) => value.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for SavedField<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(|value| Self(Some(value)))
    }
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

pub(super) fn encode(hold: &Hold, state: RecordState, generation: &str) -> Vec<u8> {
    let caller = hold.uploaded_by();
    let (state, was, retirement) = match state {
        RecordState::Pending => (StoredState::Pending, SavedField(None), SavedField(None)),
        RecordState::Kept => (StoredState::Kept, SavedField(None), SavedField(None)),
        RecordState::Retired { was, evidence } => (
            StoredState::Retired,
            SavedField(Some(was)),
            SavedField(Some(StoredRetirement::from(&evidence))),
        ),
    };
    serde_json::to_vec(&StoredHold {
        state,
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
        was,
        retirement,
    })
    .expect("a hold record is plain strings and numbers")
}

/// None for a contradictory, ambiguous, or impossible saved hold.
pub(super) fn decode(bytes: &[u8]) -> Option<HoldRecord> {
    let record: StoredHold = serde_json::from_slice(bytes).ok()?;
    let state = match (record.state, record.was.0, record.retirement.0) {
        (StoredState::Pending, None, None) => RecordState::Pending,
        (StoredState::Kept, None, None) => RecordState::Kept,
        (StoredState::Retired, Some(was), Some(retirement)) => RecordState::Retired {
            was,
            evidence: retirement.restore()?,
        },
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
    if let RecordState::Retired { was, evidence } = &state {
        RetiredHold::new(hold.clone(), *was, evidence.clone())?;
    }
    Some(HoldRecord {
        hold,
        state,
        generation: record.generation,
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredRelease {
    cause: StoredReleaseCause,
    caller: StoredCaller,
    requested_at_ms: u64,
}
impl From<&ReleaseEvidence> for StoredRelease {
    fn from(evidence: &ReleaseEvidence) -> Self {
        Self {
            cause: evidence.cause.into(),
            caller: StoredCaller {
                principal_id: evidence.caller.principal_id().as_str().into(),
                surface_id: evidence.caller.surface_id().into(),
                action_id: evidence.caller.action_id().into(),
            },
            requested_at_ms: evidence.requested_at_ms,
        }
    }
}
impl StoredRelease {
    fn restore(self) -> Option<ReleaseEvidence> {
        Some(ReleaseEvidence {
            cause: self.cause.into(),
            caller: Caller::new(
                PrincipalId::new(self.caller.principal_id).ok()?,
                &self.caller.surface_id,
                &self.caller.action_id,
            )
            .ok()?,
            requested_at_ms: self.requested_at_ms,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoredRetirement {
    Release {
        evidence: StoredRelease,
    },
    RevertedUpload {
        cause: StoredRevertCause,
        caller: StoredCaller,
    },
}
impl From<&RetirementEvidence> for StoredRetirement {
    fn from(value: &RetirementEvidence) -> Self {
        match value {
            RetirementEvidence::Release(evidence) => Self::Release {
                evidence: evidence.into(),
            },
            RetirementEvidence::RevertedUpload { cause, caller } => Self::RevertedUpload {
                cause: (*cause).into(),
                caller: StoredCaller {
                    principal_id: caller.principal_id().as_str().into(),
                    surface_id: caller.surface_id().into(),
                    action_id: caller.action_id().into(),
                },
            },
        }
    }
}
impl StoredRetirement {
    fn restore(self) -> Option<RetirementEvidence> {
        Some(match self {
            Self::Release { evidence } => RetirementEvidence::Release(evidence.restore()?),
            Self::RevertedUpload { cause, caller } => RetirementEvidence::RevertedUpload {
                cause: cause.into(),
                caller: Caller::new(
                    PrincipalId::new(caller.principal_id).ok()?,
                    &caller.surface_id,
                    &caller.action_id,
                )
                .ok()?,
            },
        })
    }
}

/// Single persistence/audit representation of an upload reversal cause.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StoredRevertCause {
    AuditUnconfirmed,
    ConfirmationFailed,
    RemovedBeforeUsable,
    UploadUnresolved,
    ConversationDeleted,
    ConversationNotFound,
}
impl From<RevertCause> for StoredRevertCause {
    fn from(cause: RevertCause) -> Self {
        match cause {
            RevertCause::AuditUnconfirmed => Self::AuditUnconfirmed,
            RevertCause::ConfirmationFailed => Self::ConfirmationFailed,
            RevertCause::RemovedBeforeUsable => Self::RemovedBeforeUsable,
            RevertCause::UploadUnresolved => Self::UploadUnresolved,
            RevertCause::ConversationDeleted => Self::ConversationDeleted,
            RevertCause::ConversationNotFound => Self::ConversationNotFound,
        }
    }
}
impl From<StoredRevertCause> for RevertCause {
    fn from(cause: StoredRevertCause) -> Self {
        match cause {
            StoredRevertCause::AuditUnconfirmed => Self::AuditUnconfirmed,
            StoredRevertCause::ConfirmationFailed => Self::ConfirmationFailed,
            StoredRevertCause::RemovedBeforeUsable => Self::RemovedBeforeUsable,
            StoredRevertCause::UploadUnresolved => Self::UploadUnresolved,
            StoredRevertCause::ConversationDeleted => Self::ConversationDeleted,
            StoredRevertCause::ConversationNotFound => Self::ConversationNotFound,
        }
    }
}

/// Single persistence/audit representation of an explicit release cause.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StoredReleaseCause {
    ConversationClosed,
    ConversationDeleted,
}
impl From<ReleaseCause> for StoredReleaseCause {
    fn from(cause: ReleaseCause) -> Self {
        match cause {
            ReleaseCause::ConversationClosed => Self::ConversationClosed,
            ReleaseCause::ConversationDeleted => Self::ConversationDeleted,
        }
    }
}
impl From<StoredReleaseCause> for ReleaseCause {
    fn from(cause: StoredReleaseCause) -> Self {
        match cause {
            StoredReleaseCause::ConversationClosed => Self::ConversationClosed,
            StoredReleaseCause::ConversationDeleted => Self::ConversationDeleted,
        }
    }
}
