//! The identity retrofit's local adapters: the durable audit, the marker file,
//! and the store's list of every conversation.
use super::{DurableIdentityRetrofitAudit, FileIdentityRetrofitMarker, IDENTITY_RETROFIT_MARKER};
use crate::agents::domain::AgentId;
use crate::conversation::application::identity_retrofit::{
    IdentityRetrofitAudit, IdentityRetrofitMarker, Leftover, PermanentLeftover,
    RetrofitAuditRecord, RetrofitCause, RetrofitConversations, RetrofitInitiator, RetrofitMove,
    RetrofitSummary, TransientLeftover,
};
use crate::conversation::application::{ConversationError, ConversationRepository};
use crate::conversation::domain::{
    Conversation, ConversationApprovalMode, ConversationDeletion, ConversationId,
    ConversationModelId,
};
use crate::conversation::infrastructure::LocalConversationStore;
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
use serde_json::Value;
use std::path::Path;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use uuid::Uuid;

struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

fn id() -> ConversationId {
    ConversationId::new(&Uuid::new_v4().to_string()).unwrap()
}

fn moved(conversation: &ConversationId) -> RetrofitMove {
    RetrofitMove {
        conversation_id: conversation.clone(),
        before: ProviderIdentity::new("claude-acp", "model", "sha256:previous").unwrap(),
        after: ProviderIdentity::new("claude-acp", "model", "sha256:current").unwrap(),
        cause: RetrofitCause::McpServersLeftRestorationIdentity,
        initiator: RetrofitInitiator::SystemGatewayStart,
    }
}

fn records(directory: &Path) -> Vec<Value> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect()
}

#[tokio::test]
async fn retrofit_evidence_is_immutable_attributed_and_each_run_keeps_its_summary() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("fingerprint-retrofit");
    let audit = DurableIdentityRetrofitAudit::new(
        directory.clone(),
        Arc::new(TestClock(AtomicU64::new(7))),
    )
    .unwrap();
    let conversation = id();
    // The same intent recorded again — a retried run — is acknowledged, not
    // duplicated.
    for _ in 0..2 {
        audit
            .record(RetrofitAuditRecord::Rewriting(moved(&conversation)))
            .await
            .unwrap();
    }
    audit
        .record(RetrofitAuditRecord::Rewritten(moved(&conversation)))
        .await
        .unwrap();
    audit
        .record(RetrofitAuditRecord::Left {
            attempted: moved(&conversation),
            reason: Leftover::Transient(TransientLeftover::Storage),
        })
        .await
        .unwrap();
    let summary = RetrofitSummary {
        rewritten: 1,
        left: vec![(
            id(),
            Leftover::Permanent(PermanentLeftover::ModelUnavailable),
        )],
        ..RetrofitSummary::default()
    };
    for _ in 0..2 {
        audit
            .record(RetrofitAuditRecord::Summary(summary.clone()))
            .await
            .unwrap();
    }
    let stored = records(&directory);
    assert_eq!(stored.len(), 5);
    let intent = stored
        .iter()
        .find(|record| record["transition"]["phase"] == "rewriting")
        .unwrap();
    assert_eq!(intent["target"]["conversationId"], conversation.to_string());
    assert_eq!(intent["transition"]["before"]["context"], "sha256:previous");
    assert_eq!(intent["transition"]["after"]["context"], "sha256:current");
    assert_eq!(intent["cause"], "mcp_servers_left_restoration_identity");
    assert_eq!(intent["initiator"]["kind"], "system");
    assert_eq!(intent["initiator"]["event"], "gateway_start");
    assert!(stored
        .iter()
        .any(|record| record["transition"]["phase"] == "left"
            && record["transition"]["left"]["kind"] == "transient"
            && record["transition"]["left"]["reason"] == "storage"));
    let summaries: Vec<_> = stored
        .iter()
        .filter(|record| record["kind"] == "conversation_restoration_identity_run")
        .collect();
    assert_eq!(summaries.len(), 2);
    assert!(summaries
        .iter()
        .all(|record| record["counts"]["rewritten"] == 1
            && record["left"][0]["left"]["reason"] == "model_unavailable"
            && record["cause"] == intent["cause"]
            && record["initiator"] == intent["initiator"]));
}

#[tokio::test]
async fn a_record_contradicting_the_one_stored_under_its_key_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("fingerprint-retrofit");
    let audit = DurableIdentityRetrofitAudit::new(
        directory.clone(),
        Arc::new(TestClock(AtomicU64::new(1))),
    )
    .unwrap();
    let conversation = id();
    audit
        .record(RetrofitAuditRecord::Rewriting(moved(&conversation)))
        .await
        .unwrap();
    let path = std::fs::read_dir(&directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["transition"]["after"]["context"] = "sha256:tampered".into();
    std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
    assert!(matches!(
        audit
            .record(RetrofitAuditRecord::Rewriting(moved(&conversation)))
            .await,
        Err(ConversationError::Audit)
    ));
}

#[tokio::test]
async fn the_marker_is_absent_until_written_and_written_once() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("retrofit");
    let marker = FileIdentityRetrofitMarker::new(directory.clone());
    assert!(!marker.done().await.unwrap());
    // Asking created nothing.
    assert!(!directory.exists());
    marker.mark_done().await.unwrap();
    assert!(directory.join(IDENTITY_RETROFIT_MARKER).is_file());
    assert!(marker.done().await.unwrap());
    marker.mark_done().await.unwrap();
    assert!(marker.done().await.unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn a_marker_that_cannot_be_read_or_written_is_unavailable_not_absent() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    // A directory where the marker would be: neither a marker nor absent.
    let directory = root.path().join("retrofit");
    std::fs::create_dir_all(directory.join(IDENTITY_RETROFIT_MARKER)).unwrap();
    let marker = FileIdentityRetrofitMarker::new(directory.clone());
    assert!(marker.done().await.is_err());
    // A directory the marker cannot be written into.
    let locked = root.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert!(FileIdentityRetrofitMarker::new(locked.join("retrofit"))
        .mark_done()
        .await
        .is_err());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[tokio::test]
async fn every_conversation_is_listed_deleted_ones_included_and_unnamed_rows_counted() {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let store = LocalConversationStore::open(&path).unwrap();
    let mut created = Vec::new();
    for _ in 0..3 {
        let conversation = id();
        store
            .create(
                Conversation::new(
                    conversation.clone(),
                    OrganizationId::new("org").unwrap(),
                    PrincipalId::new("alice").unwrap(),
                    "panel".into(),
                    "create".into(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        created.push(conversation);
    }
    store
        .record_deletion(
            &created[0],
            ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("alice").unwrap(),
                "panel".into(),
                "delete".into(),
                5,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    nessa_local_database::rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE conversations SET id = 'not a conversation' WHERE id = ?1",
            [created[2].to_string()],
        )
        .unwrap();
    let mut every = store.every_conversation().await.unwrap();
    every.conversations.sort_by_key(|id| id.to_string());
    let mut expected = created[..2].to_vec();
    expected.sort_by_key(|id| id.to_string());
    assert_eq!(every.conversations, expected);
    assert_eq!(every.unreadable, 1);
}
