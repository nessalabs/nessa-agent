//! Durable evidence that a message pointed an agent at files on this machine,
//! committed before the submission is admitted.
//!
//! An uploaded image already has a record of who put it here, written by the
//! attachments context when the bytes arrived. A path is uploaded nowhere, so
//! without this there would be no record at all that Nessa handed an agent a
//! path — possibly one outside the single directory the agent was launched in.
//! That is the reach this file exists to keep evidence of.
//!
//! ```text
//!   submit ──record──▶ this adapter ──▶ <root>/file-links/<name>.json
//!      │                                          │
//!      └── refuses the submission on failure      └── one file per submission,
//!                                                     private, named from a
//!                                                     digest of the identity
//! ```
//!
//! Arrows are calls and writes. One record per conversation and submission, so
//! a retried submission reconciles the same evidence rather than claiming a
//! second grant; contradictory evidence for the same submission fails closed,
//! exactly as conversation creation does.
//!
//! The name is derived, never copied from input: a submission identity is 256
//! bytes of whatever a caller sent, and a path is what a person chose, so
//! neither may reach the filesystem as a name.
use super::evidence_record::{commit_evidence, digest_of};
#[cfg(test)]
use super::evidence_record::{stored_evidence, Stored};
use crate::application::{
    ConversationError, ConversationFileLinkAudit, ConversationFileLinkAuditRecord,
    ConversationFileLinkCause, ConversationFileLinkState, ConversationFuture,
};
use nessa_local_storage::create_directory;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Most bytes one stored record may be before it is treated as damaged. Ten
/// paths of 4 KiB each, their JSON escaping, and the envelope around them fit
/// inside this with room to spare.
const MAX_RECORD_BYTES: usize = 131_072;

pub struct DurableConversationFileLinkAudit {
    directory: PathBuf,
}

impl DurableConversationFileLinkAudit {
    pub fn new(directory: PathBuf) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory })
    }
}

impl ConversationFileLinkAudit for DurableConversationFileLinkAudit {
    fn record(&self, record: ConversationFileLinkAuditRecord) -> ConversationFuture<'_, ()> {
        // The submission identity is the caller's text, so it is hashed rather
        // than spelled: the record still names one submission, and nothing a
        // caller writes becomes part of a path.
        let id = format!(
            "conversation-files-{}-{}",
            record.conversation_id,
            digest_of(record.execution_id.as_bytes())
        );
        let value = json!({
            "recordId": id,
            "kind": "conversation_files_named",
            "target": {
                "conversationId": record.conversation_id.to_string(),
                "organizationId": record.organization_id.as_str(),
                "executionId": record.execution_id,
                "paths": record.paths,
            },
            "transition": {
                "before": state(record.before),
                "after": state(record.after),
            },
            "cause": cause(record.cause),
            "initiator": {
                "principalId": record.initiator_principal_id.as_str(),
                "surfaceId": record.initiator_surface_id,
            },
            "observedAtMs": record.observed_at_ms,
        });
        self.commit(id, value)
    }
    fn record_input(
        &self,
        record: crate::application::InputFileAudit,
    ) -> ConversationFuture<'_, ()> {
        let id = format!(
            "input-file-{}-{}-{:?}",
            record.conversation,
            digest_of(record.attempt.as_bytes()),
            record.stage
        );
        let value = json!({
            "recordId": id, "kind": "conversation_input_file", "conversationId": record.conversation.to_string(), "organizationId": record.organization.as_str(), "executionId": record.execution, "leaseId": record.lease, "attempt": record.attempt,
            "file": { "digest": record.file.digest().to_string(), "mimeType": record.file.media_type().as_str(), "size": record.file.size(), "name": record.file.name() },
            "stage": format!("{:?}", record.stage), "destination": record.destination, "delivered": record.delivered, "failure": record.failure.map(|failure| format!("{failure:?}")),
            "initiator": record.initiator.map(|(principal, surface)| json!({ "principalId": principal.as_str(), "surfaceId": surface })),
            "cause": if record.stage == crate::application::InputFileStage::Submitted { "caller_submitted" } else { "automatic_input_delivery" }, "observedAtMs": record.observed_at_ms,
        });
        self.commit(id, value)
    }
}
impl DurableConversationFileLinkAudit {
    fn commit(&self, id: String, value: Value) -> ConversationFuture<'_, ()> {
        commit_evidence(self.directory.clone(), id, value, MAX_RECORD_BYTES)
    }
}

fn state(value: ConversationFileLinkState) -> &'static str {
    match value {
        ConversationFileLinkState::NotNamed => "not_named",
        ConversationFileLinkState::Named => "named",
    }
}

fn cause(value: ConversationFileLinkCause) -> &'static str {
    match value {
        ConversationFileLinkCause::CallerSubmitted => "caller_submitted",
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/file_link_audit.rs"]
mod tests;
