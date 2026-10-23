//! Who may write the deletion inventory, and the fences a committed
//! command records there. Restore and quarantine are not this capture.

use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::backup::application::{
    capture_inventory, CaptureError, DecisionAuditError, DeletionLogStore, FenceSource,
    InventoryAction, InventoryCapturePolicyEvaluator, InventoryCause, InventoryDecision,
    InventoryDecisionAudit, InventoryDenial, InventoryFacts, InventoryFailure, LogStoreError,
};
use crate::backup::domain::{DeletionFence, DeletionLog};
use crate::backup::infrastructure::{CedarInventoryPolicy, CommittedInventory};
use crate::conversation::application::{
    AccessAsk, AccessDecision, ConversationAccessPolicyEvaluator, ConversationCaller,
    ConversationError, ConversationRepository, FenceCapture, FenceCaptureError, ReceiverAuthority,
    ShareConversation, ShareGrantChange, ShareGrantTransition, ShareGrants,
};
use crate::conversation::domain::{Capabilities, Conversation, ReceiverBinding, ShareAccess};
use crate::conversation::infrastructure::LocalConversationStore;
use nessa_auth::application::dto::{CredentialGrantDto, CredentialMetadataDto, ResourceDto};
use nessa_auth::application::ports::{AccessReader, AccessSnapshot, Clock, PortFuture};
use nessa_auth::domain::{
    Action, AudienceId, Credential, CredentialId, Grant, Membership, MembershipId, MembershipRole,
    MembershipStatus, OrganizationId, PrincipalId, Resource, ResourceId,
};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_protocol::conversation::read_scope::ReadRefusal;

const VERIFIER: &str = "VERIFIER-MUST-NOT-LEAK";

fn open_inventory(root: &Path) -> CommittedInventory {
    CommittedInventory::open(root, Arc::new(FixedClock)).unwrap()
}

struct FixedClock;

impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        1
    }
}

fn decisions(root: &Path) -> String {
    std::fs::read_to_string(root.join("backup/inventory-decisions.jsonl")).unwrap()
}

#[tokio::test]
async fn the_gateway_may_record_a_committed_fence() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    write_registry(
        root,
        vec![credential("device-1", "user:one", "server.read")],
    );
    let inventory = open_inventory(root);
    FenceCapture::capture(&inventory).await.unwrap();
    let log = std::fs::read_to_string(root.join("backup/deletion-inventory.txt")).unwrap();
    assert!(log.contains("credential device device-1"), "{log}");
    assert!(!log.contains(VERIFIER), "{log}");
    let decision = CedarInventoryPolicy::new()
        .unwrap()
        .evaluate(&InventoryFacts {
            kind: "gateway",
            cause: InventoryCause::CommittedFence,
            action: InventoryAction::Record,
        });
    let InventoryDecision::Allow { policies } = decision else {
        panic!("the gateway was not allowed to record: {decision:?}");
    };
    assert!(
        policies.iter().any(|id| id == "inventory.record.gateway"),
        "{policies:?}"
    );
    let stored = decisions(root);
    assert!(stored.contains("\"outcome\":\"allow\""), "{stored}");
    assert!(stored.contains("inventory.record.gateway"), "{stored}");
    assert!(stored.contains("\"principalKind\":\"gateway\""), "{stored}");
    assert!(stored.contains("\"cause\":\"committed-fence\""), "{stored}");
    assert!(
        stored.contains("\"action\":\"deletion.inventory.record\""),
        "{stored}"
    );
    assert!(stored.contains("\"observedAtMs\":1"), "{stored}");
    assert!(!stored.contains("principalId"), "{stored}");
}

#[tokio::test]
async fn a_device_peer_environment_or_worker_may_not_capture() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let inventory = open_inventory(root);
    for kind in ["device", "peer", "environment-peer", "worker"] {
        let error = inventory.capture_as(kind).await.unwrap_err();
        let CaptureError::Denied(InventoryDenial::Forbidden { policies }) = error else {
            panic!("{kind} was not forbidden: {error:?}");
        };
        assert!(
            policies
                .iter()
                .any(|id| id == &format!("inventory.refuse.{kind}")),
            "{kind}: {policies:?}"
        );
    }
    assert!(!root.join("backup/deletion-inventory.txt").exists());
    let stored = decisions(root);
    for kind in ["device", "peer", "environment-peer", "worker"] {
        assert!(
            stored.contains(&format!("inventory.refuse.{kind}")),
            "{kind}: {stored}"
        );
    }
    assert!(stored.contains("\"outcome\":\"deny\""), "{stored}");
    assert!(stored.contains("\"denial\":\"forbidden\""), "{stored}");
    let policy = CedarInventoryPolicy::new().unwrap();
    assert!(matches!(
        policy.evaluate(&facts("person", InventoryCause::Operator, InventoryAction::Capture)),
        InventoryDecision::Allow { policies } if policies.iter().any(|id| id == "inventory.capture.person")
    ));
    assert!(matches!(
        policy.evaluate(&facts("surface", InventoryCause::Operator, InventoryAction::Capture)),
        InventoryDecision::Allow { policies } if policies.iter().any(|id| id == "inventory.capture.surface")
    ));
    assert!(matches!(
        policy.evaluate(&facts(
            "gateway",
            InventoryCause::Operator,
            InventoryAction::Capture
        )),
        InventoryDecision::Deny(InventoryDenial::NotPermitted)
    ));
    assert!(matches!(
        policy.evaluate(&facts(
            "person",
            InventoryCause::CommittedFence,
            InventoryAction::Record
        )),
        InventoryDecision::Deny(InventoryDenial::NotPermitted)
    ));
    assert!(matches!(
        policy.evaluate(&facts(
            "device",
            InventoryCause::CommittedFence,
            InventoryAction::Record
        )),
        InventoryDecision::Deny(InventoryDenial::Forbidden { .. })
    ));
}

#[tokio::test]
async fn a_denied_capture_does_not_write() {
    let touched = Touched(AtomicBool::new(false));
    let log = MemLog(Mutex::new(DeletionLog::default()));
    let error = capture_inventory(
        &facts("device", InventoryCause::Operator, InventoryAction::Capture),
        &Refuse,
        &Remember,
        &[&touched],
        &log,
    )
    .await
    .unwrap_err();
    assert_eq!(error, CaptureError::Denied(InventoryDenial::Unavailable));
    assert!(!touched.0.load(Ordering::SeqCst));
    assert!(log.0.lock().unwrap().entries().is_empty());

    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    write_registry(
        root,
        vec![credential("device-1", "user:one", "server.read")],
    );
    let inventory = open_inventory(root);
    FenceCapture::capture(&inventory).await.unwrap();
    let path = root.join("backup/deletion-inventory.txt");
    let before = std::fs::read(&path).unwrap();
    let denied = inventory.capture_as("device").await.unwrap_err();
    assert!(matches!(
        denied,
        CaptureError::Denied(InventoryDenial::Forbidden { .. })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let stored = decisions(root);
    assert!(stored.contains("inventory.refuse.device"), "{stored}");
    assert!(stored.contains("\"denial\":\"forbidden\""), "{stored}");
}

#[tokio::test]
async fn the_inventory_decision_is_stored_before_the_log() {
    let policy = CedarInventoryPolicy::new().unwrap();
    let order = Order::default();
    let recorded = capture_inventory(
        &facts(
            "gateway",
            InventoryCause::CommittedFence,
            InventoryAction::Record,
        ),
        &policy,
        &order,
        &[&order],
        &order,
    )
    .await
    .unwrap();
    assert!(order.log_after_audit.load(Ordering::SeqCst));
    assert_eq!(recorded.entries().len(), 1);
    assert!(recorded.canonical().contains("conversation gone"));
}

#[tokio::test]
async fn a_failed_decision_audit_does_not_write() {
    let touched = Touched(AtomicBool::new(false));
    let log = MemLog(Mutex::new(DeletionLog::default()));
    let error = capture_inventory(
        &facts("person", InventoryCause::Operator, InventoryAction::Capture),
        &CedarInventoryPolicy::new().unwrap(),
        &Closed,
        &[&touched],
        &log,
    )
    .await
    .unwrap_err();
    assert_eq!(error, CaptureError::NotAudited);
    assert!(!touched.0.load(Ordering::SeqCst));
    assert!(log.0.lock().unwrap().entries().is_empty());

    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    write_registry(
        root,
        vec![credential("device-1", "user:one", "server.read")],
    );
    let inventory = open_inventory(root);
    std::fs::create_dir(root.join("backup/inventory-decisions.jsonl")).unwrap();
    let error = inventory.capture_as("person").await.unwrap_err();
    assert_eq!(error, CaptureError::NotAudited);
    assert!(!root.join("backup/deletion-inventory.txt").exists());
}

#[tokio::test]
async fn a_revoked_worker_credential_is_inventoried() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    write_registry(
        root,
        vec![
            credential("device-1", "user:one", "server.read"),
            credential("peer-1", "gateway:peer", "conversation.read"),
            credential("env-1", "user:env", "environment.serve"),
            credential("worker-1", "user:worker", "environment.serve"),
        ],
    );
    write_worker(root, "workers", "one.json", "worker-1");
    write_worker(root, "workers", "device.json", "device-1");
    let inventory = open_inventory(root);
    FenceCapture::capture(&inventory).await.unwrap();
    let log = std::fs::read_to_string(root.join("backup/deletion-inventory.txt")).unwrap();
    assert!(log.contains("credential device device-1"), "{log}");
    assert!(log.contains("credential peer peer-1"), "{log}");
    assert!(log.contains("credential environment-peer env-1"), "{log}");
    assert!(log.contains("credential worker worker-1"), "{log}");
    assert!(!log.contains("credential worker device-1"), "{log}");
    assert!(!log.contains(VERIFIER), "{log}");
}

#[tokio::test]
async fn a_damaged_worker_enrollment_does_not_commit_fences() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    write_registry(
        root,
        vec![credential("device-1", "user:one", "server.read")],
    );
    write_worker_bytes(
        root,
        "workers",
        "bad.json",
        b"{\"name\":\"not-a-credential\"}",
    );
    let inventory = open_inventory(root);
    let error = inventory.capture_as("person").await.unwrap_err();
    assert_eq!(error, CaptureError::Inventory(InventoryFailure::Unreadable));
    assert!(!root.join("backup/deletion-inventory.txt").exists());

    let blocked = tempfile::tempdir().unwrap();
    let root = blocked.path();
    write_registry(
        root,
        vec![credential("device-1", "user:one", "server.read")],
    );
    std::fs::write(root.join("workers"), b"not-a-directory").unwrap();
    let inventory = open_inventory(root);
    let error = inventory.capture_as("person").await.unwrap_err();
    assert_eq!(error, CaptureError::Inventory(InventoryFailure::Unreadable));
    assert!(!root.join("backup/deletion-inventory.txt").exists());
}

#[tokio::test]
async fn an_unshare_reconciles_the_revoked_grant() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let private = root.join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = LocalConversationStore::open(&private.join("metadata.sqlite3"))
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    store.create(owned(&id)).await.unwrap();
    assert!(
        store
            .change(grant_change(
                &id,
                ShareGrantTransition::Grant(ShareAccess::read()),
                Some("read-party"),
                "grant",
            ))
            .await
            .unwrap()
            .applied
    );
    let inventory = open_inventory(root);
    let unused = Unused;
    let gateway = resource();
    let shares = ShareConversation {
        conversations: &store,
        receivers: &unused,
        grants: &store,
        access: &unused,
        gateway: &gateway,
        policy: &unused,
        inventory: Some(&inventory),
    };
    assert!(shares
        .unshare(
            caller("unshare"),
            id,
            CredentialId::new("read-party-credential").unwrap(),
            None,
            20,
        )
        .await
        .unwrap());
    let log = std::fs::read_to_string(root.join("backup/deletion-inventory.txt")).unwrap();
    assert!(log.contains(" grant "), "{log}");
    assert!(log.contains("read-party"), "{log}");
}

/// A committed regrant survives capture failure; an equivalent new request repairs the
/// inventory from persisted grants rather than retaining the old revoke.
#[tokio::test]
async fn a_retried_share_reconciles_after_a_failed_capture() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let private = root.join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let policy = crate::conversation_test_support::access_policy();
    let store = LocalConversationStore::open(&private.join("metadata.sqlite3"))
        .unwrap()
        .with_access_policy(policy.clone());
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    store.create(owned(&id)).await.unwrap();
    for (transition, receiver, request) in [
        (
            ShareGrantTransition::Grant(ShareAccess::read()),
            Some("read-party"),
            "grant",
        ),
        (ShareGrantTransition::Revoke, None, "revoke"),
    ] {
        assert!(
            store
                .change(grant_change(&id, transition, receiver, request))
                .await
                .unwrap()
                .applied
        );
    }
    let inventory = open_inventory(root);
    FenceCapture::capture(&inventory).await.unwrap();
    let inventory_path = root.join("backup/deletion-inventory.txt");
    let revoked = std::fs::read_to_string(&inventory_path).unwrap();
    assert!(revoked.contains(" grant "), "{revoked}");
    let capture = FailFirstCapture {
        inventory,
        calls: AtomicUsize::new(0),
    };
    let target = ShareTarget;
    let gateway = resource();
    let shares = ShareConversation {
        conversations: &store,
        receivers: &target,
        grants: &store,
        access: &target,
        gateway: &gateway,
        policy: policy.as_ref(),
        inventory: Some(&capture),
    };
    let credential = CredentialId::new("read-party-credential").unwrap();
    let first = shares
        .share(
            caller("regrant"),
            id.clone(),
            credential.clone(),
            Capabilities::read(),
            None,
            None,
            30,
        )
        .await
        .unwrap();
    assert!(first.applied);
    assert_eq!(store.grants(&id).await.unwrap().grants.len(), 1);
    assert_eq!(std::fs::read_to_string(&inventory_path).unwrap(), revoked);
    let retry = shares
        .share(
            caller("retry-regrant"),
            id.clone(),
            credential.clone(),
            Capabilities::read(),
            None,
            None,
            30,
        )
        .await
        .unwrap();
    assert!(!retry.applied);
    assert_eq!(retry.policy_revision, first.policy_revision);
    assert_eq!(capture.calls.load(Ordering::SeqCst), 2);
    let reconciled = std::fs::read_to_string(&inventory_path).unwrap();
    assert!(!reconciled.contains(" grant "), "{reconciled}");
    assert_eq!(store.grants(&id).await.unwrap().grants.len(), 1);

    // Ownership denial and a failed conditional write cannot trigger capture.
    let mut denied = caller("denied");
    denied.principal_id = PrincipalId::new("other").unwrap();
    assert!(matches!(
        shares
            .share(
                denied,
                id.clone(),
                credential.clone(),
                Capabilities::read(),
                None,
                None,
                40
            )
            .await,
        Err(ConversationError::NotFound)
    ));
    assert!(matches!(
        shares
            .share(
                caller("stale"),
                id,
                credential,
                Capabilities::read(),
                None,
                Some(0),
                40
            )
            .await,
        Err(ConversationError::ShareGrantChanged)
    ));
    assert_eq!(capture.calls.load(Ordering::SeqCst), 2);
}

struct FailFirstCapture {
    inventory: CommittedInventory,
    calls: AtomicUsize,
}

#[async_trait]
impl FenceCapture for FailFirstCapture {
    async fn capture(&self) -> Result<(), FenceCaptureError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(FenceCaptureError::NotStored);
        }
        FenceCapture::capture(&self.inventory).await
    }
}

struct ShareTarget;

impl ReceiverAuthority for ShareTarget {
    fn resolve<'a>(
        &'a self,
        credential: &'a CredentialId,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            Ok(Some(ReceiverBinding {
                receiver_id: "read-party".into(),
                credential_id: credential.clone(),
                organization_id: OrganizationId::new("org").unwrap(),
                owner_id: PrincipalId::new("alice").unwrap(),
                access_epoch: 1,
                active: true,
            }))
        })
    }
}

impl AccessReader for ShareTarget {
    fn read<'a>(&'a self, credential: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async move {
            let org = OrganizationId::new("org").unwrap();
            let principal = PrincipalId::new("grantee-read-party").unwrap();
            Ok({
                let credential = Credential::new(
                    credential.clone(),
                    principal.clone(),
                    org.clone(),
                    AudienceId::new("gateway").unwrap(),
                    1,
                    100,
                    vec![Grant::new(
                        Action::new("conversation.read").unwrap(),
                        resource(),
                    )],
                )
                .unwrap();
                let membership = Membership::new(
                    MembershipId::new("member").unwrap(),
                    principal,
                    org,
                    MembershipRole::Member,
                    MembershipStatus::Active,
                );
                let revision = 1;
                // This fixture explicitly creates its own original product issuance.
                let original = nessa_auth::domain::CredentialTransition::new(
                    credential.id().clone(),
                    None,
                    nessa_auth::domain::CredentialLifecycle {
                        issued_at: credential.issued_at(),
                        expires_at: credential.expires_at(),
                        revoked_at: None,
                    },
                    nessa_auth::domain::TransitionCause::Issued(
                        nessa_auth::domain::IssuanceCause::AdminIssue,
                    ),
                    nessa_auth::domain::Initiator::LocalOperator,
                    credential.issued_at(),
                )
                .unwrap();
                let origin = nessa_auth::application::origin::CredentialOrigin::restore(
                    nessa_auth::application::dto::CredentialTransitionDto::record(
                        &original, 1, 1, None,
                    ),
                    &credential,
                )
                .unwrap();
                AccessSnapshot {
                    credential,
                    membership,
                    revision,
                    origin,
                }
            })
        })
    }
}

struct FailingCapture;

#[async_trait]
impl FenceCapture for FailingCapture {
    async fn capture(&self) -> Result<(), FenceCaptureError> {
        Err(FenceCaptureError::NotStored)
    }
}

/// A capture that cannot store the log does not undo the revoke. The grant
/// stays gone and `unshare` still answers that it applied.
#[tokio::test]
async fn a_failed_inventory_capture_leaves_the_revoke() {
    use crate::conversation::application::ShareGrants;
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let private = root.join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = LocalConversationStore::open(&private.join("metadata.sqlite3"))
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    store.create(owned(&id)).await.unwrap();
    assert!(
        store
            .change(grant_change(
                &id,
                ShareGrantTransition::Grant(ShareAccess::read()),
                Some("read-party"),
                "grant",
            ))
            .await
            .unwrap()
            .applied
    );
    let failing = FailingCapture;
    let unused = Unused;
    let gateway = resource();
    let shares = ShareConversation {
        conversations: &store,
        receivers: &unused,
        grants: &store,
        access: &unused,
        gateway: &gateway,
        policy: &unused,
        inventory: Some(&failing),
    };
    assert!(shares
        .unshare(
            caller("unshare"),
            id.clone(),
            CredentialId::new("read-party-credential").unwrap(),
            None,
            20,
        )
        .await
        .unwrap());
    assert!(store.grants(&id).await.unwrap().grants.is_empty());
}

struct Refuse;

impl InventoryCapturePolicyEvaluator for Refuse {
    fn evaluate(&self, _facts: &InventoryFacts<'_>) -> InventoryDecision {
        InventoryDecision::Deny(InventoryDenial::Unavailable)
    }
}

struct Remember;

impl InventoryDecisionAudit for Remember {
    fn record(
        &self,
        _: &InventoryFacts<'_>,
        decision: &InventoryDecision,
    ) -> Result<(), DecisionAuditError> {
        assert_eq!(
            decision,
            &InventoryDecision::Deny(InventoryDenial::Unavailable)
        );
        Ok(())
    }
}

struct Closed;

impl InventoryDecisionAudit for Closed {
    fn record(
        &self,
        _: &InventoryFacts<'_>,
        _: &InventoryDecision,
    ) -> Result<(), DecisionAuditError> {
        Err(DecisionAuditError::NotStored)
    }
}

struct Order {
    audited: AtomicBool,
    log_after_audit: AtomicBool,
    log: Mutex<DeletionLog>,
}

impl Default for Order {
    fn default() -> Self {
        Self {
            audited: AtomicBool::new(false),
            log_after_audit: AtomicBool::new(false),
            log: Mutex::new(DeletionLog::default()),
        }
    }
}

impl InventoryDecisionAudit for Order {
    fn record(
        &self,
        facts: &InventoryFacts<'_>,
        decision: &InventoryDecision,
    ) -> Result<(), DecisionAuditError> {
        assert_eq!(facts.kind, "gateway");
        assert_eq!(facts.cause, InventoryCause::CommittedFence);
        assert!(self.log.lock().unwrap().entries().is_empty());
        let InventoryDecision::Allow { policies } = decision else {
            panic!("the gateway was not allowed: {decision:?}");
        };
        assert!(
            policies.iter().any(|id| id == "inventory.record.gateway"),
            "{policies:?}"
        );
        self.audited.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl FenceSource for Order {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        assert!(self.audited.load(Ordering::SeqCst));
        Ok(vec![DeletionFence::conversation("gone").unwrap()])
    }
}

#[async_trait]
impl DeletionLogStore for Order {
    async fn hold_reconciliation(
        &self,
    ) -> Result<Box<dyn crate::backup::application::InventoryReconciliationLease>, ()> {
        Ok(Box::new(UncontendedReconciliation))
    }

    async fn load(&self) -> Result<DeletionLog, ()> {
        assert!(self.audited.load(Ordering::SeqCst));
        self.log_after_audit.store(true, Ordering::SeqCst);
        Ok(self.log.lock().unwrap().clone())
    }

    async fn store(&self, base: &DeletionLog, next: &DeletionLog) -> Result<(), LogStoreError> {
        assert!(self.audited.load(Ordering::SeqCst));
        let mut guard = self.log.lock().unwrap();
        if guard.canonical() != base.canonical() {
            return Err(LogStoreError::Stale);
        }
        *guard = next.clone();
        Ok(())
    }
}

struct Touched(AtomicBool);

#[async_trait]
impl FenceSource for Touched {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        self.0.store(true, Ordering::SeqCst);
        Ok(vec![])
    }
}

struct MemLog(Mutex<DeletionLog>);

#[async_trait]
impl DeletionLogStore for MemLog {
    async fn hold_reconciliation(
        &self,
    ) -> Result<Box<dyn crate::backup::application::InventoryReconciliationLease>, ()> {
        Ok(Box::new(UncontendedReconciliation))
    }

    async fn load(&self) -> Result<DeletionLog, ()> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn store(&self, base: &DeletionLog, next: &DeletionLog) -> Result<(), LogStoreError> {
        let mut guard = self.0.lock().unwrap();
        if guard.canonical() != base.canonical() {
            return Err(LogStoreError::Stale);
        }
        *guard = next.clone();
        Ok(())
    }
}

struct Unused;

impl ConversationAccessPolicyEvaluator for Unused {
    fn evaluate(&self, _ask: &AccessAsk<'_>) -> AccessDecision {
        unreachable!("unshare does not ask the access policy")
    }
}

impl ReceiverAuthority for Unused {
    fn resolve<'a>(
        &'a self,
        _credential_id: &'a CredentialId,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>>
                + Send
                + 'a,
        >,
    > {
        unreachable!("unshare does not resolve a receiver")
    }
}

impl AccessReader for Unused {
    fn read<'a>(&'a self, _credential: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        unreachable!("unshare does not read a credential")
    }
}

fn facts(kind: &str, cause: InventoryCause, action: InventoryAction) -> InventoryFacts<'_> {
    InventoryFacts {
        kind,
        cause,
        action,
    }
}

fn credential(id: &str, principal: &str, action: &str) -> serde_json::Value {
    let metadata = CredentialMetadataDto {
        id: id.into(),
        principal_id: principal.into(),
        organization_id: "org".into(),
        audience_id: "gateway".into(),
        issued_at: 1,
        expires_at: None,
        revoked_at: Some(9),
        grants: vec![CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: "org".into(),
                id: "gateway".into(),
            },
        }],
    };
    serde_json::json!({ "metadata": metadata, "verifier": VERIFIER })
}

fn write_registry(root: &Path, rows: Vec<serde_json::Value>) {
    let directory = root.join("auth");
    nessa_local_storage::create_directory(&directory).unwrap();
    std::fs::write(
        directory.join("credentials.v1.json"),
        serde_json::to_vec(&serde_json::json!({ "credentials": rows })).unwrap(),
    )
    .unwrap();
}

fn write_worker(root: &Path, directory: &str, name: &str, id: &str) {
    write_worker_bytes(
        root,
        directory,
        name,
        serde_json::json!({ "credentialId": id })
            .to_string()
            .as_bytes(),
    );
}

fn write_worker_bytes(root: &Path, directory: &str, name: &str, bytes: &[u8]) {
    let directory = root.join(directory);
    nessa_local_storage::create_directory(&directory).unwrap();
    std::fs::write(directory.join(name), bytes).unwrap();
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

fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("alice").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}

fn grant_change(
    id: &ConversationId,
    transition: ShareGrantTransition,
    receiver: Option<&str>,
    action: &str,
) -> ShareGrantChange {
    ShareGrantChange {
        transition,
        conversation_id: id.clone(),
        principal_id: receiver.map(|id| PrincipalId::new(format!("grantee-{id}")).unwrap()),
        receiver_id: receiver.map(str::to_owned),
        credential_id: CredentialId::new("read-party-credential").unwrap(),
        expected_policy_revision: None,
        initiator: caller(action),
        at_ms: 10,
    }
}

fn resource() -> Resource {
    Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    )
}

// These adapters are used by single-capture tests only.
struct UncontendedReconciliation;

impl crate::backup::application::InventoryReconciliationLease for UncontendedReconciliation {}
