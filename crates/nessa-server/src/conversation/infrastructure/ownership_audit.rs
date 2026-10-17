//! Immutable ownership transition evidence using the existing durable audit writer.
use super::mode_audit::write_once;
use crate::conversation::application::ConversationError;
use async_trait::async_trait;
use nessa_sdk::application::agent_execution::subagents::{OwnershipAudit, PortFailure};
use nessa_sdk::domain::agent_execution::subagents::{Initiator, OwnershipEvidence};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub struct DurableOwnershipAudit {
    directory: PathBuf,
}
impl DurableOwnershipAudit {
    pub fn new(directory: PathBuf) -> Result<Self, ConversationError> {
        nessa_local_storage::create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory })
    }
}
#[async_trait]
impl OwnershipAudit for DurableOwnershipAudit {
    async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
        let initiator = match &evidence.initiator {
            Initiator::Host(actor) => {
                json!({"kind":"host", "principalId":actor.principal_id(), "surfaceId":actor.surface_id(), "requestId":actor.request_id()})
            }
            Initiator::Runtime => json!({"kind":"runtime"}),
        };
        let value = json!({"kind":"owned_thread_transition", "parentLifetime":evidence.parent_lifetime.as_str(),
            "childLifetime":evidence.child_lifetime.as_ref().map(|id| id.as_str()),
            "closeOperation":evidence.close_operation.as_ref().map(|id| id.as_str()),
            "before":format!("{:?}", evidence.before), "after":format!("{:?}", evidence.after),
            "cause":evidence.cause.as_ref().map(|cause| format!("{cause:?}")),
            "closeDetail":evidence.close_detail.as_ref().map(|detail| format!("{detail:?}")), "initiator":initiator});
        let bytes = serde_json::to_vec(&value).map_err(|_| PortFailure::Rejected)?;
        let id = format!("ownership-{:x}", Sha256::digest(bytes));
        // A failed physical acknowledgement is uncertain; do not promise rollback.
        write_once(self.directory.clone(), id, value)
            .await
            .map_err(|_| PortFailure::Uncertain)
    }
}
