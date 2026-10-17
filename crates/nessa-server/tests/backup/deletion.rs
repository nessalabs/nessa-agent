//! Deletion inventory: revoked grants and credentials, idempotent fences,
//! and a failed append that leaves the previous log.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::backup::application::{
    capture_fences, DeletionLogStore, ExportClock, FenceSource, InventoryFailure, LogStoreError,
    MutationAdmission,
};
use crate::backup::domain::{CredentialKind, DeletionFence, DeletionLog, Fingerprint, GrantRole};
use crate::backup::infrastructure::read_credentials;
use crate::backup::infrastructure::{capture_namespace, DirectoryPublisher, FileDeletionLog};
use crate::conversation::application::{
    ConversationRepository, ShareGrantChange, ShareGrantTransition, ShareGrants,
};
use crate::conversation::domain::{
    Capabilities, Conversation, ShareAccess, ToolPolicy, ToolPreset,
};
use crate::conversation::infrastructure::{ExportSkip, LocalConversationStore, MetadataSnapshot};
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
        ShareAccess::new(Capabilities::comment(), None).unwrap(),
    )
    .await;
    grant(
        &opened.store,
        &drive,
        "drive-party",
        ShareAccess::new(
            Capabilities::drive(),
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
            Capabilities::drive(),
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
        .map(|revocation| revocation.capabilities.as_str())
        .collect();
    assert_eq!(roles, ["read", "read+comment", "read+comment+drive"]);
    assert_eq!(facts.grants.len(), 1);
    let live = String::from_utf8(facts.grants[0].bytes.clone()).unwrap();
    assert!(live.contains("\"capabilities\":\"read+comment+drive\""));
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
async fn a_regrant_drops_the_revoke_fence_and_keeps_other_fences() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    grant(&opened.store, &id, "party", ShareAccess::read()).await;
    revoke(&opened.store, &id, "party").await;
    let revoked = GrantFences {
        facts: opened.store.export_facts().await.unwrap(),
    };
    let credential = DeletionFence::credential(
        CredentialKind::Device,
        "device-kept",
        Fingerprint::of(b"device-kept"),
    )
    .unwrap();
    let log = MemLog::default();
    let recorded = capture_fences(&[&revoked, &OneFence(credential.clone())], &log)
        .await
        .unwrap();
    assert_eq!(recorded.entries().len(), 2);
    grant(&opened.store, &id, "party", ShareAccess::read()).await;
    let current = GrantFences {
        facts: opened.store.export_facts().await.unwrap(),
    };
    assert!(current.facts.revocations.is_empty());
    let again = capture_fences(&[&current], &log).await.unwrap();
    assert_eq!(again.entries().len(), 1);
    assert_eq!(again.entries()[0].fence().key(), credential.key());
    assert!(!again.canonical().contains(" grant "));
}

#[tokio::test]
async fn a_failed_fence_source_does_not_commit_the_other_source() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let private = root.join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = LocalConversationStore::open(&private.join("metadata.sqlite3"))
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    grant(&store, &id, "party", ShareAccess::read()).await;
    revoke(&store, &id, "party").await;
    fs_create(root.join("auth").as_path());
    std::fs::write(root.join("auth/credentials.v1.json"), b"not-a-registry").unwrap();
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let manifest = capture_namespace(
        root,
        &FenceAdmission,
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &FenceClock,
        &inventory.key(),
    )
    .await
    .unwrap();
    let text = manifest.encoded();
    assert!(text.contains("skipped deletion-inventory append unavailable"));
    assert!(!text.contains("party"));
    let log =
        std::fs::read_to_string(root.join("backup/deletion-inventory.txt")).unwrap_or_default();
    assert!(!log.contains("party"));
    assert!(!log.contains("grant "));
}

#[tokio::test]
async fn a_damaged_fence_row_leaves_the_stored_revoke() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let private = root.join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let metadata = private.join("metadata.sqlite3");
    let store = LocalConversationStore::open(&metadata)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    let kept = new_id();
    let damaged = new_id();
    store.create(owned(&kept)).await.unwrap();
    store.create(owned(&damaged)).await.unwrap();
    grant(&store, &kept, "kept-party", ShareAccess::read()).await;
    revoke(&store, &kept, "kept-party").await;
    grant(&store, &damaged, "damaged-party", ShareAccess::read()).await;
    revoke(&store, &damaged, "damaged-party").await;
    drop(store);

    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let first_bundle = tempfile::tempdir().unwrap();
    let first = capture_namespace(
        root,
        &FenceAdmission,
        &inventory,
        &DirectoryPublisher::new(first_bundle.path().join("cut")),
        &FenceClock,
        &inventory.key(),
    )
    .await
    .unwrap();
    assert!(!first
        .encoded()
        .contains("skipped deletion-inventory append unavailable"));
    let log_path = root.join("backup/deletion-inventory.txt");
    let before = std::fs::read_to_string(&log_path).unwrap();
    assert!(before.contains("kept-party"));
    assert!(before.contains("damaged-party"));

    let connection = nessa_local_database::rusqlite::Connection::open(&metadata).unwrap();
    let changed = connection
        .execute(
            "UPDATE share_grant_changes
             SET receiver_id = receiver_id || char(1)
             WHERE receiver_id = 'damaged-party'",
            [],
        )
        .unwrap();
    assert!(changed >= 1);
    drop(connection);

    let snapshot = MetadataSnapshot::open(&metadata).unwrap();
    assert!(snapshot.fences().is_err());
    let facts = snapshot.facts().unwrap();
    assert!(facts
        .revocations
        .iter()
        .any(|revocation| revocation.receiver_id == "kept-party"));
    assert!(facts
        .revocations
        .iter()
        .all(|revocation| revocation.receiver_id != "damaged-party"));
    assert!(facts
        .skipped
        .iter()
        .any(|skip| matches!(skip, ExportSkip::Grant { .. })));
    drop(snapshot);

    let second_bundle = tempfile::tempdir().unwrap();
    let second = capture_namespace(
        root,
        &FenceAdmission,
        &inventory,
        &DirectoryPublisher::new(second_bundle.path().join("cut")),
        &FenceClock,
        &inventory.key(),
    )
    .await
    .unwrap();
    let text = second.encoded();
    assert!(text.contains("skipped deletion-inventory append unavailable"));
    assert!(text.contains("skipped share-grants revoke-"));
    assert_eq!(std::fs::read_to_string(&log_path).unwrap(), before);
}

#[tokio::test]
async fn a_skipped_credential_row_does_not_commit_fences() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    fs_create(root.join("auth").as_path());
    let registry = serde_json::json!({
        "credentials": [
            {"metadata": {
                "id": "kept-device",
                "principalId": "user:one",
                "organizationId": "org",
                "audienceId": "gateway",
                "issuedAt": 1,
                "expiresAt": null,
                "revokedAt": 9,
                "grants": [{"action": "server.read", "resource": {"organizationId": "org", "id": "gateway"}}]
            }, "verifier": "SECRET-VERIFIER"},
            {"metadata": "not-a-credential"}
        ]
    });
    std::fs::write(
        root.join("auth/credentials.v1.json"),
        serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let manifest = capture_namespace(
        root,
        &FenceAdmission,
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &FenceClock,
        &inventory.key(),
    )
    .await
    .unwrap();
    let text = manifest.encoded();
    assert!(text.contains("skipped deletion-inventory append unavailable"));
    assert!(text.contains("skipped device-credentials row-2"));
    let log =
        std::fs::read_to_string(root.join("backup/deletion-inventory.txt")).unwrap_or_default();
    assert!(!log.contains("kept-device"));
    assert!(!log.contains("credential "));
}

fn fs_create(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
}

struct FenceAdmission;

#[async_trait]
impl MutationAdmission for FenceAdmission {
    async fn hold(
        &self,
    ) -> Result<
        Box<dyn crate::backup::application::AdmissionLease + Send + Sync>,
        crate::backup::application::AdmissionFailure,
    > {
        Ok(Box::new(FenceHold))
    }
}

struct FenceHold;

impl crate::backup::application::AdmissionLease for FenceHold {
    fn retirement_signaled(&self) -> bool {
        false
    }
}

struct FenceClock;

impl ExportClock for FenceClock {
    fn unix_ms(&self) -> u64 {
        1
    }
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
async fn a_stale_log_is_reconciled_again() {
    let other = DeletionFence::conversation("other-writer").unwrap();
    let ours = DeletionFence::conversation("this-writer").unwrap();
    let log = StaleOnce::new(other.clone());
    let recorded = capture_fences(&[&OneFence(ours.clone())], &log)
        .await
        .unwrap();
    let keys: Vec<&str> = recorded
        .entries()
        .iter()
        .map(|entry| entry.fence().key())
        .collect();
    assert!(keys.contains(&other.key()));
    assert!(keys.contains(&ours.key()));
}

#[tokio::test]
async fn a_failed_reconciliation_lease_does_not_read_sources_or_the_log() {
    let current = ObservedEmpty(AtomicBool::new(false));
    assert_eq!(
        capture_fences(&[&current], &RefusedReconciliation).await,
        Err(InventoryFailure::NotStored)
    );
    assert!(!current.0.load(Ordering::SeqCst));
}

struct RefusedReconciliation;

#[async_trait]
impl DeletionLogStore for RefusedReconciliation {
    async fn hold_reconciliation(
        &self,
    ) -> Result<Box<dyn crate::backup::application::InventoryReconciliationLease>, ()> {
        Err(())
    }

    async fn load(&self) -> Result<DeletionLog, ()> {
        panic!("a failed reconciliation lease must stop before loading")
    }

    async fn store(&self, _base: &DeletionLog, _next: &DeletionLog) -> Result<(), LogStoreError> {
        panic!("a failed reconciliation lease must stop before storing")
    }
}

/// The first read captured a revoke before a re-grant. Without a lease,
/// the re-grant's empty-to-empty capture returns, then the older read can
/// store an obsolete revoke against that unchanged empty base.
#[tokio::test]
async fn a_noop_regrant_capture_waits_for_an_older_revoke_read() {
    let revoke = DeletionFence::share_grant("c", "r", "cred", GrantRole::Read, 4).unwrap();
    let older = PausedRevoke {
        revoke,
        entered: AtomicBool::new(false),
        release: tokio::sync::Notify::new(),
    };
    let current = ObservedEmpty(AtomicBool::new(false));
    let log = MemLog::default();
    let older_sources: [&dyn FenceSource; 1] = [&older];
    let current_sources: [&dyn FenceSource; 1] = [&current];
    let mut first = std::pin::pin!(capture_fences(&older_sources, &log));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(first.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(older.entered.load(Ordering::SeqCst));

    let mut second = std::pin::pin!(capture_fences(&current_sources, &log));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(second.as_mut(), cx).is_pending(),
            "the newer no-op capture returned while an older source read was pending"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert!(!current.0.load(Ordering::SeqCst));
    older.release.notify_one();
    assert_eq!(first.await.unwrap().entries().len(), 1);
    assert!(second.await.unwrap().entries().is_empty());
    assert!(current.0.load(Ordering::SeqCst));
    assert!(log.load().await.unwrap().entries().is_empty());
}

struct PausedRevoke {
    revoke: DeletionFence,
    entered: AtomicBool,
    release: tokio::sync::Notify,
}

#[async_trait]
impl FenceSource for PausedRevoke {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        let snapshot = self.revoke.clone();
        self.entered.store(true, Ordering::SeqCst);
        self.release.notified().await;
        Ok(vec![snapshot])
    }
}

struct ObservedEmpty(AtomicBool);

#[async_trait]
impl FenceSource for ObservedEmpty {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        self.0.store(true, Ordering::SeqCst);
        Ok(vec![])
    }
}

/// Another capture stores a share-grant revoke as this writer loads. The
/// fence source is empty until that store is visible. Reading fences before
/// the load would drop the revoke; reading them after the load keeps it.
#[tokio::test]
async fn a_concurrent_revoke_is_not_dropped_by_a_stale_fence_snapshot() {
    let revoke = DeletionFence::share_grant("c", "r", "cred", GrantRole::Read, 4).unwrap();
    let source = RevokeLandsOnLoad::new(revoke.clone());
    let recorded = capture_fences(&[&source], &source).await.unwrap();
    let keys: Vec<&str> = recorded
        .entries()
        .iter()
        .map(|entry| entry.fence().key())
        .collect();
    assert!(keys.contains(&revoke.key()), "{keys:?}");
}

/// The revoke is absent from the fence source until `load` has observed the
/// log that already holds it.
struct RevokeLandsOnLoad {
    log: Mutex<DeletionLog>,
    revoke: DeletionFence,
    visible: AtomicBool,
}

impl RevokeLandsOnLoad {
    fn new(revoke: DeletionFence) -> Self {
        Self {
            log: Mutex::new(DeletionLog::default()),
            revoke,
            visible: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl FenceSource for RevokeLandsOnLoad {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        if self.visible.load(Ordering::SeqCst) {
            Ok(vec![self.revoke.clone()])
        } else {
            Ok(vec![])
        }
    }
}

#[async_trait]
impl DeletionLogStore for RevokeLandsOnLoad {
    async fn hold_reconciliation(
        &self,
    ) -> Result<Box<dyn crate::backup::application::InventoryReconciliationLease>, ()> {
        Ok(Box::new(UncontendedReconciliation))
    }

    async fn load(&self) -> Result<DeletionLog, ()> {
        let mut guard = self.log.lock().unwrap();
        if guard.entries().is_empty() {
            let (landed, _) = DeletionLog::default().append(self.revoke.clone());
            *guard = landed;
            self.visible.store(true, Ordering::SeqCst);
        }
        Ok(guard.clone())
    }

    async fn store(&self, base: &DeletionLog, next: &DeletionLog) -> Result<(), LogStoreError> {
        let mut guard = self.log.lock().unwrap();
        if guard.canonical() != base.canonical() {
            return Err(LogStoreError::Stale);
        }
        *guard = next.clone();
        Ok(())
    }
}

/// The first `store` reports [`LogStoreError::Stale`] after another fence
/// landed. The retry has to keep that fence.
struct StaleOnce {
    log: Mutex<DeletionLog>,
    other: Mutex<Option<DeletionFence>>,
}

impl StaleOnce {
    fn new(other: DeletionFence) -> Self {
        Self {
            log: Mutex::new(DeletionLog::default()),
            other: Mutex::new(Some(other)),
        }
    }
}

#[async_trait]
impl DeletionLogStore for StaleOnce {
    async fn hold_reconciliation(
        &self,
    ) -> Result<Box<dyn crate::backup::application::InventoryReconciliationLease>, ()> {
        Ok(Box::new(UncontendedReconciliation))
    }

    async fn load(&self) -> Result<DeletionLog, ()> {
        Ok(self.log.lock().unwrap().clone())
    }

    async fn store(&self, base: &DeletionLog, next: &DeletionLog) -> Result<(), LogStoreError> {
        let mut guard = self.log.lock().unwrap();
        if let Some(other) = self.other.lock().unwrap().take() {
            let (landed, _) = DeletionLog::default().append(other);
            *guard = landed;
            return Err(LogStoreError::Stale);
        }
        if guard.canonical() != base.canonical() {
            return Err(LogStoreError::Stale);
        }
        *guard = next.clone();
        Ok(())
    }
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
                    GrantRole::parse(&revocation.capabilities).unwrap(),
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
    reconciliation: std::sync::Arc<tokio::sync::Mutex<()>>,
    log: Mutex<DeletionLog>,
    fail: AtomicBool,
}

#[async_trait]
impl DeletionLogStore for MemLog {
    async fn hold_reconciliation(
        &self,
    ) -> Result<Box<dyn crate::backup::application::InventoryReconciliationLease>, ()> {
        Ok(Box::new(HeldReconciliation {
            _guard: self.reconciliation.clone().lock_owned().await,
        }))
    }

    async fn load(&self) -> Result<DeletionLog, ()> {
        Ok(self.log.lock().unwrap().clone())
    }

    async fn store(&self, base: &DeletionLog, next: &DeletionLog) -> Result<(), LogStoreError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(LogStoreError::Failed);
        }
        let mut guard = self.log.lock().unwrap();
        if guard.canonical() != base.canonical() {
            return Err(LogStoreError::Stale);
        }
        *guard = next.clone();
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
        principal_id: receiver.map(|id| PrincipalId::new(format!("grantee-{id}")).unwrap()),
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

// These adapters are used by single-capture tests only.
struct UncontendedReconciliation;

impl crate::backup::application::InventoryReconciliationLease for UncontendedReconciliation {}

struct HeldReconciliation {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl crate::backup::application::InventoryReconciliationLease for HeldReconciliation {}
