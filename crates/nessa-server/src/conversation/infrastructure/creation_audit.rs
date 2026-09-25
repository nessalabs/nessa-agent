//! Durable conversation-creation evidence, committed before creation is acknowledged.
use crate::conversation::application::{
    ConversationCreationAudit, ConversationCreationAuditRecord, ConversationCreationCause,
    ConversationError, ConversationFuture, ConversationOwnershipState,
};
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde_json::json;
use std::{
    io::{Read, Write},
    path::PathBuf,
};
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
        let initial = record.cause == ConversationCreationCause::CallerRequested;
        let id = if initial {
            format!("conversation-created-{}", record.conversation_id)
        } else {
            Uuid::new_v4().to_string()
        };
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
            "observedAtMs": record.observed_at_ms,
        });
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let destination = directory.join(format!("{id}.json"));
                if initial {
                    match open(&destination, OpenMode::Read) {
                        Ok(mut file) => {
                            let mut bytes = Vec::new();
                            Read::by_ref(&mut file)
                                .take(16_385)
                                .read_to_end(&mut bytes)
                                .map_err(|_| ConversationError::Audit)?;
                            if bytes.len() > 16_384 {
                                return Err(ConversationError::Audit);
                            }
                            let mut stored: serde_json::Value = serde_json::from_slice(&bytes)
                                .map_err(|_| ConversationError::Audit)?;
                            let mut expected = value.clone();
                            let _ = stored
                                .as_object_mut()
                                .and_then(|value| value.remove("observedAtMs"));
                            let _ = expected
                                .as_object_mut()
                                .and_then(|value| value.remove("observedAtMs"));
                            if stored != expected {
                                return Err(ConversationError::Audit);
                            }
                            return sync_directory(&directory)
                                .map_err(|_| ConversationError::Audit);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => return Err(ConversationError::Audit),
                    }
                }
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|_| ConversationError::Audit)?;
                serde_json::to_writer(file.as_file_mut(), &value)
                    .map_err(|_| ConversationError::Audit)?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|_| file.as_file().sync_all())
                    .map_err(|_| ConversationError::Audit)?;
                file.persist(&destination)
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
        ConversationOwnershipState::Deleted => "deleted",
    }
}

fn cause(value: ConversationCreationCause) -> &'static str {
    match value {
        ConversationCreationCause::CallerRequested => "caller_requested",
        ConversationCreationCause::IdempotentReopen => "idempotent_reopen",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{application::ConversationOwnershipState, domain::ConversationId};
    use nessa_auth::domain::{OrganizationId, PrincipalId};

    fn creation(observed_at_ms: u64) -> ConversationCreationAuditRecord {
        ConversationCreationAuditRecord {
            conversation_id: ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            organization_id: OrganizationId::new("org").unwrap(),
            owner_id: PrincipalId::new("owner").unwrap(),
            before: ConversationOwnershipState::Absent,
            after: ConversationOwnershipState::Owned,
            cause: ConversationCreationCause::CallerRequested,
            initiator_principal_id: PrincipalId::new("owner").unwrap(),
            initiator_surface_id: "panel".into(),
            correlation_id: "create-1".into(),
            requested_at_ms: 100,
            observed_at_ms,
        }
    }

    #[tokio::test]
    async fn initial_creation_is_idempotent_but_conflicting_evidence_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let audit = DurableConversationCreationAudit::new(root.path().join("creation")).unwrap();

        audit.record(creation(110)).await.unwrap();
        audit.record(creation(120)).await.unwrap();

        assert_eq!(
            std::fs::read_dir(root.path().join("creation"))
                .unwrap()
                .count(),
            1
        );
        let mut conflict = creation(130);
        conflict.correlation_id = "different".into();
        assert!(matches!(
            audit.record(conflict).await,
            Err(ConversationError::Audit)
        ));
    }
}
