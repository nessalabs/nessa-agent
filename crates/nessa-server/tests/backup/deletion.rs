//! Deletion inventory: revoked grants and credentials, idempotent fences,
//! and a failed append that leaves the previous log.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::backup::application::{capture_fences, DeletionLogStore, FenceSource, InventoryFailure};
use crate::backup::domain::{CredentialKind, DeletionFence, DeletionLog, Fingerprint, GrantRole};
use crate::backup::infrastructure::read_credentials;
use crate::conversation::application::{
    ConversationRepository, ShareGrantChange, ShareGrantTransition, ShareGrants,
};
use crate::conversation::domain::{Conversation, ShareAccess, ShareRole, ToolPolicy, ToolPreset};
use crate::conversation::infrastructure::LocalConversationStore;
use nessa_auth::application::dto::{CredentialGrantDto, CredentialMetadataDto, ResourceDto};
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};

#[tokio::test]
async fn revoked_grants_of_every_role_are_inventoried() {
    let opened = opened();
    let read = new_id();
    let comment = new_id();
    let drive = new_id();
    let kept = new_id();
    for id in [&read, &comment, &drive, &kept] {
        opened.store.create(owned(id)).await.unwrap();
    }
    grant(&opened.store, &read, "read-party", ShareAccess::read()).await;
    grant(
        &opened.store,
        &comment,
        "comment-party",
        ShareAccess::new(ShareRole::Comment, None).unwrap(),
    )
    .await;
    grant(
        &opened.store,
        &drive,
        "drive-party",
        ShareAccess::new(
            ShareRole::Drive,
            Some(ToolPolicy::preset(ToolPreset::ReadOnlyTools)),
        )
        .unwrap(),
    )
    .await;
    grant(
        &opened.store,
        &kept,
        "kept-party",
        ShareAccess::new(
            ShareRole::Drive,
            Some(ToolPolicy::preset(ToolPreset::NoShell)),
        )
        .unwrap(),
    )
    .await;
    revoke(&opened.store, &read, "read-party").await;
    revoke(&opened.store, &comment, "comment-party").await;
    revoke(&opened.store, &drive, "drive-party").await;

    let facts = opened.store.export_facts().await.unwrap();
    let roles: Vec<_> = facts
        .revocations
        .iter()
        .map(|revocation| revocation.role.as_str())
        .collect();
    assert_eq!(roles, ["read", "comment", "drive"]);
    assert_eq!(facts.grants.len(), 1);
    let live = String::from_utf8(facts.grants[0].bytes.clone()).unwrap();
    assert!(live.contains("\"role\":\"drive\""));
    assert!(live.contains("policyRevision"));
    assert!(live.contains("kept-party"));

    let source = GrantFences {
        facts: facts.clone(),
    };
    let log = MemLog::default();
    let recorded = capture_fences(&[&source], &log).await.unwrap();
    assert_eq!(recorded.entries().len(), 3);
    let again = capture_fences(&[&source], &log).await.unwrap();
    assert_eq!(again.entries().len(), 3);
    assert_eq!(again.entries()[0].sequence(), 1);
}

#[test]
fn revoked_credentials_are_inventoried_without_secrets() {
    let verifier = "VERIFIER-MUST-NOT-LEAK";
    let metadata = |id: &str, principal: &str, action: &str, revoked: bool| CredentialMetadataDto {
        id: id.into(),
        principal_id: principal.into(),
        organization_id: "org".into(),
        audience_id: "gateway".into(),
        issued_at: 1,
        expires_at: None,
        revoked_at: revoked.then_some(9),
        grants: vec![CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: "org".into(),
                id: "gateway".into(),
            },
        }],
    };
    let file = serde_json::json!({
        "credentials": [
            {"metadata": metadata("device-1", "user:one", "server.read", true), "verifier": verifier},
            {"metadata": metadata("peer-1", "gateway:peer", "conversation.read", true), "verifier": verifier},
            {"metadata": metadata("env-1", "user:env", "environment.serve", true), "verifier": verifier},
            {"metadata": "not-a-credential", "verifier": "OTHER-SECRET"}
        ]
    });
    let read = read_credentials(serde_json::to_vec(&file).unwrap().as_slice()).unwrap();
    assert_eq!(read.skipped, ["row-4"]);
    let kinds: Vec<_> = read
        .credentials
        .iter()
        .map(|credential| credential.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            CredentialKind::Device,
            CredentialKind::Peer,
            CredentialKind::EnvironmentPeer
        ]
    );
    assert!(read.credentials.iter().all(|credential| credential.revoked));
    assert!(read
        .credentials
        .iter()
        .all(|credential| credential.kind != CredentialKind::Worker));
    let encoded = serde_json::to_string(
        &read
            .credentials
            .iter()
            .map(|credential| String::from_utf8(credential.record.clone()).unwrap())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!encoded.contains(verifier));
    assert!(!encoded.contains("OTHER-SECRET"));
    assert!(!encoded.contains("verifier"));
    let fence = DeletionFence::credential(
        CredentialKind::Worker,
        "worker-1",
        Fingerprint::of(b"worker-1"),
    )
    .unwrap();
    assert!(!fence.kind().canonical_probe().contains(verifier));
    assert_eq!(
        CredentialKind::from_registry("gateway:peer", &["environment.serve".into()]),
        CredentialKind::EnvironmentPeer,
        "one registry row is an environment peer, not also a worker"
    );
}

#[tokio::test]
async fn a_repeated_fence_does_not_advance_the_sequence() {
    let log = DeletionLog::default();
    let fence = DeletionFence::conversation("conversation-1").unwrap();
    let (once, first) = log.append(fence.clone());
    let (twice, second) = once.append(fence);
    assert_eq!(once.entries().len(), 1);
    assert_eq!(twice.entries().len(), 1);
    assert_eq!(
        first,
        crate::backup::domain::Append::Recorded { sequence: 1 }
    );
    assert_eq!(
        second,
        crate::backup::domain::Append::AlreadyRecorded { sequence: 1 }
    );
}

#[tokio::test]
async fn a_failed_inventory_append_leaves_the_fence_and_the_log() {
    let fence = DeletionFence::share_grant("c", "r", "cred", GrantRole::Read, 4).unwrap();
    let source = OneFence(fence.clone());
    let log = MemLog::default();
    capture_fences(&[&source], &log).await.unwrap();
    log.fail.store(true, Ordering::SeqCst);
    let error = capture_fences(
        &[&OneFence(DeletionFence::conversation("other").unwrap())],
        &log,
    )
    .await
    .unwrap_err();
    assert_eq!(error, InventoryFailure::NotStored);
    let kept = log.load().await.unwrap();
    assert_eq!(kept.entries().len(), 1);
    assert_eq!(kept.entries()[0].fence().key(), fence.key());
}

#[tokio::test]
async fn the_cut_includes_the_deletion_inventory() {
    let fence = DeletionFence::conversation("gone").unwrap();
    let source = OneFence(fence);
    let log = MemLog::default();
    let recorded = capture_fences(&[&source], &log).await.unwrap();
    assert_eq!(recorded.watermark(), "1");
    assert!(recorded.canonical().contains("conversation gone"));
}

struct GrantFences {
    facts: crate::conversation::infrastructure::ExportFacts,
}

#[async_trait]
impl FenceSource for GrantFences {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        self.facts
            .revocations
            .iter()
            .map(|revocation| {
                DeletionFence::share_grant(
                    &revocation.conversation_id,
                    &revocation.receiver_id,
                    &revocation.credential_id,
                    GrantRole::parse(&revocation.role).unwrap(),
                    revocation.sequence,
                )
                .map_err(|_| ())
            })
            .collect()
    }
}

struct OneFence(DeletionFence);

#[async_trait]
impl FenceSource for OneFence {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        Ok(vec![self.0.clone()])
    }
}

#[derive(Default)]
struct MemLog {
    log: Mutex<DeletionLog>,
    fail: AtomicBool,
}

#[async_trait]
impl DeletionLogStore for MemLog {
    async fn load(&self) -> Result<DeletionLog, ()> {
        Ok(self.log.lock().unwrap().clone())
    }

    async fn store(&self, next: &DeletionLog) -> Result<(), ()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(());
        }
        *self.log.lock().unwrap() = next.clone();
        Ok(())
    }
}

fn opened() -> Opened {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = LocalConversationStore::open(&private.join("metadata.sqlite3"))
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    Opened {
        _directory: directory,
        store,
    }
}

struct Opened {
    _directory: tempfile::TempDir,
    store: LocalConversationStore,
}

fn new_id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}

fn owned(id: &ConversationId) -> Conversation {
    Conversation::new(
        id.clone(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("alice").unwrap(),
        "panel".into(),
        "create".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("test-model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap()
}

async fn grant(
    store: &LocalConversationStore,
    id: &ConversationId,
    receiver: &str,
    access: ShareAccess,
) {
    let receipt = store
        .change(change(
            ShareGrantTransition::Grant(access),
            id,
            Some(receiver),
            receiver,
        ))
        .await
        .unwrap();
    assert!(receipt.applied);
}

async fn revoke(store: &LocalConversationStore, id: &ConversationId, receiver: &str) {
    let receipt = store
        .change(change(ShareGrantTransition::Revoke, id, None, receiver))
        .await
        .unwrap();
    assert!(receipt.applied);
}

fn change(
    transition: ShareGrantTransition,
    id: &ConversationId,
    receiver: Option<&str>,
    credential: &str,
) -> ShareGrantChange {
    ShareGrantChange {
        transition,
        conversation_id: id.clone(),
        receiver_id: receiver.map(str::to_owned),
        credential_id: CredentialId::new(format!("{credential}-credential")).unwrap(),
        expected_policy_revision: None,
        initiator: crate::conversation::application::ConversationCaller {
            organization_id: OrganizationId::new("org").unwrap(),
            principal_id: PrincipalId::new("alice").unwrap(),
            surface_id: "desktop".into(),
            action_id: uuid::Uuid::new_v4().to_string(),
        },
        at_ms: 7,
    }
}

trait CanonicalProbe {
    fn canonical_probe(&self) -> String;
}

impl CanonicalProbe for crate::backup::domain::FenceKind {
    fn canonical_probe(&self) -> String {
        format!("{self:?}")
    }
}
