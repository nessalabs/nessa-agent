//! Durable conversation-deletion evidence, committed before any history is erased.
//!
//! One record per conversation, named for it: a conversation is deleted once,
//! so a repeated delete acknowledges the same record again, and a record that
//! contradicts the stored one in anything but when it was observed is refused
//! rather than written over
//! (`a_repeated_deletion_is_acknowledged_and_a_contradicting_one_fails_closed`).
//! Private, synced, and published whole. Nothing here removes a record, and a
//! delete leaves the other audit stores as they were
//! (`deleting_on_the_local_stores_erases_what_it_owns_and_leaves_every_audit_record`).
//!
//! When the record was observed is this adapter's reading of its injected
//! clock as it builds the record. It is not a claim about when anything was
//! erased, which happens after the record is acknowledged.
use crate::conversation::{
    application::{
        ConversationDeletionAudit, ConversationDeletionAuditRecord, ConversationDeletionCause,
        ConversationError, ConversationFuture, ConversationOwnershipState,
    },
    domain::ProviderSessionErasure,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde_json::json;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct DurableConversationDeletionAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableConversationDeletionAudit {
    /// Keep records in the private `directory`, observed by `clock`.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}

impl ConversationDeletionAudit for DurableConversationDeletionAudit {
    fn record(&self, record: ConversationDeletionAuditRecord) -> ConversationFuture<'_, ()> {
        let id = format!("conversation-deleted-{}", record.conversation_id);
        let value = json!({
            "recordId": id,
            "kind": "conversation_deleted",
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
            "providerSessionId": record.provider_session_id.as_ref().map(|session| session.as_str()),
            "providerErasure": provider_erasure(record.provider_erasure),
            "requestedAtMs": record.requested_at_ms,
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let destination = directory.join(format!("{id}.json"));
                if matches_stored(&destination, &id, &value)? {
                    return sync_directory(&directory).map_err(|_| ConversationError::Audit);
                }
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|_| ConversationError::Audit)?;
                serde_json::to_writer(file.as_file_mut(), &value)
                    .map_err(|_| ConversationError::Audit)?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|_| file.as_file().sync_all())
                    .map_err(|_| ConversationError::Audit)?;
                // Published without replacing. Two deletes racing to write the
                // first record meet here, and the later one is held to what the
                // earlier one wrote instead of writing over it.
                match file.publish(&destination) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        if !matches_stored(&destination, &id, &value)? {
                            return Err(ConversationError::Audit);
                        }
                    }
                    Err(_) => return Err(ConversationError::Audit),
                }
                sync_directory(&directory).map_err(|_| ConversationError::Audit)
            })
            .await
            .map_err(|_| ConversationError::Audit)?
        })
    }
}

/// Whether a record is already stored under this name: `false` when there is
/// none, `true` when there is and it says what `value` says, and a failure
/// when it cannot be read or says anything else. When it was observed is the
/// one thing a repeat may say differently.
fn matches_stored(
    destination: &Path,
    id: &str,
    value: &serde_json::Value,
) -> Result<bool, ConversationError> {
    let mut file = match open(destination, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(ConversationError::Audit),
    };
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|_| ConversationError::Audit)?;
    if bytes.len() > 16_384 {
        return Err(ConversationError::Audit);
    }
    let mut stored: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| ConversationError::Audit)?;
    let mut expected = value.clone();
    for document in [&mut stored, &mut expected] {
        if let Some(fields) = document.as_object_mut() {
            fields.remove("observedAtMs");
        }
    }
    if stored != expected {
        tracing::error!(
            record = %id,
            "a deletion record contradicts the one already stored"
        );
        return Err(ConversationError::Audit);
    }
    Ok(true)
}

fn ownership(value: ConversationOwnershipState) -> &'static str {
    match value {
        ConversationOwnershipState::Absent => "absent",
        ConversationOwnershipState::Owned => "owned",
        ConversationOwnershipState::Deleted => "deleted",
    }
}

fn provider_erasure(value: ProviderSessionErasure) -> &'static str {
    match value {
        ProviderSessionErasure::NoProviderSession => "no_provider_session",
        ProviderSessionErasure::SessionUnknown => "session_unknown",
        ProviderSessionErasure::Deleted => "deleted",
        ProviderSessionErasure::Archived => "archived",
        ProviderSessionErasure::Acknowledged => "acknowledged",
        ProviderSessionErasure::NotListed => "not_listed",
        ProviderSessionErasure::NotSupported => "not_supported",
        ProviderSessionErasure::NoHandler => "no_handler",
    }
}

fn cause(value: ConversationDeletionCause) -> &'static str {
    match value {
        ConversationDeletionCause::CallerRequested => "caller_requested",
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/deletion_audit.rs"]
mod tests;
