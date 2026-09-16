//! Durable conversation-creation evidence, committed before creation is acknowledged.
use crate::conversation::application::{
    ConversationCreationAudit, ConversationCreationAuditRecord, ConversationCreationCause,
    ConversationError, ConversationFuture, ConversationOwnershipState,
};
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::json;
use std::{io::Write, path::PathBuf};
use uuid::Uuid;

pub struct DurableConversationCreationAudit {
    directory: PathBuf,
}

impl DurableConversationCreationAudit {
    pub fn new(directory: PathBuf) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory })
    }
}

impl ConversationCreationAudit for DurableConversationCreationAudit {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()> {
        let id = Uuid::new_v4().to_string();
        let value = json!({
            "recordId": id,
            "kind": "conversation_creation_acknowledged",
            "target": {
                "conversationId": record.conversation_id.to_string(),
                "organizationId": record.organization_id.as_str(),
                "ownerId": record.owner_id.as_str(),
            },
            "transition": {
                "before": ownership(record.before),
                "after": ownership(record.after),
            },
            "cause": cause(record.cause),
            "initiator": {
                "principalId": record.initiator_principal_id.as_str(),
                "surfaceId": record.initiator_surface_id,
            },
            "correlationId": record.correlation_id,
            "requestedAtMs": record.requested_at_ms,
            "committedAtMs": record.committed_at_ms,
            "observedAtMs": record.observed_at_ms,
        });
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|_| ConversationError::Audit)?;
                serde_json::to_writer(file.as_file_mut(), &value)
                    .map_err(|_| ConversationError::Audit)?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|_| file.as_file().sync_all())
                    .map_err(|_| ConversationError::Audit)?;
                file.persist(&directory.join(format!("{id}.json")))
                    .map_err(|_| ConversationError::Audit)?;
                sync_directory(&directory).map_err(|_| ConversationError::Audit)
            })
            .await
            .map_err(|_| ConversationError::Audit)?
        })
    }
}

fn ownership(value: ConversationOwnershipState) -> &'static str {
    match value {
        ConversationOwnershipState::Absent => "absent",
        ConversationOwnershipState::Owned => "owned",
    }
}

fn cause(value: ConversationCreationCause) -> &'static str {
    match value {
        ConversationCreationCause::CallerRequested => "caller_requested",
        ConversationCreationCause::IdempotentReopen => "idempotent_reopen",
    }
}
