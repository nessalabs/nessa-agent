//! The local deletion record: one per conversation, private, and never
//! written over by a record that says something else.
use super::*;
use crate::conversation::domain::{ConversationId, ProviderSessionErasure};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use std::sync::atomic::{AtomicU64, Ordering};

/// A clock a test moves by hand.
struct ManualClock(AtomicU64);
impl Clock for ManualClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn audit_at(
    directory: PathBuf,
    now_ms: u64,
) -> (DurableConversationDeletionAudit, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock(AtomicU64::new(now_ms)));
    (
        DurableConversationDeletionAudit::new(directory, clock.clone()).unwrap(),
        clock,
    )
}

fn deletion() -> ConversationDeletionAuditRecord {
    ConversationDeletionAuditRecord {
        conversation_id: ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        organization_id: OrganizationId::new("org").unwrap(),
        owner_id: PrincipalId::new("owner").unwrap(),
        before: ConversationOwnershipState::Owned,
        after: ConversationOwnershipState::Deleted,
        cause: ConversationDeletionCause::CallerRequested,
        initiator_principal_id: PrincipalId::new("owner").unwrap(),
        initiator_surface_id: "phone".into(),
        correlation_id: "delete-1".into(),
        provider_session_id: Some(ExecutionSessionId::new("provider-session").unwrap()),
        provider_erasure: ProviderSessionErasure::Archived,
        requested_at_ms: 100,
    }
}

#[tokio::test]
async fn a_deletion_is_recorded_once_with_who_deleted_it_and_the_provider_session() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("deletion");
    let (audit, _) = audit_at(directory.clone(), 110);
    audit.record(deletion()).await.unwrap();

    let path = directory.join("conversation-deleted-00000000-0000-4000-8000-000000000001.json");
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        stored,
        json!({
            "recordId": "conversation-deleted-00000000-0000-4000-8000-000000000001",
            "kind": "conversation_deleted",
            "target": {
                "conversationId": "00000000-0000-4000-8000-000000000001",
                "organizationId": "org",
                "ownerId": "owner",
            },
            "transition": {"before": "owned", "after": "deleted"},
            "cause": "caller_requested",
            "initiator": {"principalId": "owner", "surfaceId": "phone"},
            "correlationId": "delete-1",
            "providerSessionId": "provider-session",
            "providerErasure": "archived",
            "requestedAtMs": 100,
            "observedAtMs": 110,
        })
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // A history that named no provider session says so, rather than leaving
    // the field out.
    let mut quiet = deletion();
    quiet.conversation_id = ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap();
    quiet.provider_session_id = None;
    quiet.provider_erasure = ProviderSessionErasure::NoProviderSession;
    audit.record(quiet).await.unwrap();
    let stored: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            directory.join("conversation-deleted-00000000-0000-4000-8000-000000000002.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stored["providerSessionId"], serde_json::Value::Null);
    assert_eq!(stored["providerErasure"], "no_provider_session");
    // Each outcome is written as what it is: an archive is never an erasure.
    for (erasure, written) in [
        (ProviderSessionErasure::Deleted, "deleted"),
        (ProviderSessionErasure::Acknowledged, "acknowledged"),
        (ProviderSessionErasure::NotListed, "not_listed"),
        (ProviderSessionErasure::NotSupported, "not_supported"),
        (ProviderSessionErasure::NoHandler, "no_handler"),
    ] {
        let mut record = deletion();
        record.conversation_id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
        record.provider_erasure = erasure;
        let path = directory.join(format!(
            "conversation-deleted-{}.json",
            record.conversation_id
        ));
        audit.record(record).await.unwrap();
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(stored["providerErasure"], written);
    }
}

#[tokio::test]
async fn a_repeated_deletion_is_acknowledged_and_a_contradicting_one_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("deletion");
    let (audit, clock) = audit_at(directory.clone(), 110);
    audit.record(deletion()).await.unwrap();
    let path = directory.join("conversation-deleted-00000000-0000-4000-8000-000000000001.json");
    let original = std::fs::read(&path).unwrap();

    // The same deletion seen again, later: acknowledged, and not rewritten.
    clock.0.store(120, Ordering::SeqCst);
    audit.record(deletion()).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);

    // Anything else about the one deletion is a contradiction, refused, and
    // the stored record stays as it was.
    let contradictions: [fn(&mut ConversationDeletionAuditRecord); 6] = [
        |record| record.correlation_id = "delete-2".into(),
        |record| record.initiator_surface_id = "panel".into(),
        |record| record.provider_session_id = None,
        |record| record.provider_erasure = ProviderSessionErasure::Deleted,
        |record| record.requested_at_ms = 101,
        |record| record.owner_id = PrincipalId::new("someone").unwrap(),
    ];
    for contradict in contradictions {
        let mut record = deletion();
        contradict(&mut record);
        assert!(matches!(
            audit.record(record).await,
            Err(ConversationError::Audit)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

#[tokio::test]
async fn a_deletion_record_that_cannot_be_read_or_written_is_a_visible_failure() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("deletion");
    let (audit, _) = audit_at(directory.clone(), 1);
    // Damaged where the record should be: never overwritten, never accepted.
    let path = directory.join("conversation-deleted-00000000-0000-4000-8000-000000000001.json");
    std::fs::write(&path, b"{").unwrap();
    assert!(matches!(
        audit.record(deletion()).await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"{");
    // A directory that is gone.
    std::fs::remove_dir_all(&directory).unwrap();
    let mut other = deletion();
    other.conversation_id = ConversationId::new("00000000-0000-4000-8000-000000000003").unwrap();
    assert!(matches!(
        audit.record(other).await,
        Err(ConversationError::Audit)
    ));
}
