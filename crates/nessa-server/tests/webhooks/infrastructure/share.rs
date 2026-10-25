//! Actual Auth/Cedar/private registration/grant composition; no receiver or send.
use super::source_tests::{physical, read_limits, registered};
use super::*;
use crate::conversation::application::{ConversationError, GrantedShare};
use crate::conversation::application::{RecordReadOperation, RecordReadValue};
use crate::conversation::domain::{Capabilities, ToolPolicy, ToolPreset};
use crate::conversation::infrastructure::{GrantWritePause, NessaRecordReadSource};
use crate::webhooks::application::ReadWebhookRecords;
use crate::webhooks::application::{
    OutboxError, Registration, RegistrationLookup, ShareWebhookConversation,
};
use nessa_auth::application::credential_admin::CredentialAdmin;
use nessa_auth::application::ports::AccessAdmission;
use nessa_sync::replication::domain::Id;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;

fn caller(f: &Fixture, request: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: f.target.organization.clone(),
        principal_id: f.target.owner.clone(),
        surface_id: "synthetic".into(),
        action_id: request.into(),
    }
}
async fn clean() -> Fixture {
    let f = Fixture::with_purpose(CredentialIssuePurpose::Webhook).await;
    f.change(ShareGrantTransition::Revoke, "remove-fixture-grant")
        .await;
    f
}
async fn share(
    f: &Fixture,
    lookup: &dyn RegistrationLookup,
    capabilities: Capabilities,
    policy: Option<ToolPolicy>,
) -> Result<GrantedShare, ConversationError> {
    ShareWebhookConversation {
        registrations: lookup,
        admission: f.admission(),
    }
    .execute(
        caller(f, "synthetic-share"),
        f.a.clone(),
        f.target.credential.clone(),
        capabilities,
        policy,
        None,
        2000,
    )
    .await
}
async fn absent(f: &Fixture) {
    assert!(f
        .grants
        .principal_grant(&f.a, &f.target.principal)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn registered_principal_share_has_no_receiver_and_grants_exactly_one_conversation() {
    let f = clean().await;
    let (store, registration, _) = registered(&f).await;
    let receipt = share(&f, &store, Capabilities::read(), None).await.unwrap();
    assert!(receipt.applied);
    let grant = f
        .grants
        .principal_grant(&f.a, &f.target.principal)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grant.receiver_id, None);
    assert_eq!(grant.principal_id, registration.target.principal);
    assert_eq!(grant.credential_id, registration.target.credential);
    assert_eq!(grant.policy_revision, receipt.policy_revision);
    assert_eq!(grant.access, ShareAccess::read());
    let source = NessaRecordReadSource::new(
        physical(&f).await,
        Id::new("source").unwrap(),
        Handle::current(),
    );
    let capacity = Arc::new(Semaphore::new(1));
    let records = ReadWebhookRecords {
        admission: f.admission(),
        source: &source,
        capacity: &capacity,
        limits: read_limits(),
    }
    .execute(
        &registration,
        &f.a,
        receipt.policy_revision,
        RecordReadOperation::Head,
    )
    .await
    .unwrap();
    assert!(matches!(records.value().unwrap(),RecordReadValue::Head(head) if head.head==2));
    drop(records);
    source.shutdown().await.unwrap();
    assert!(f
        .admission()
        .execute(&registration.target, &f.a, receipt.policy_revision)
        .await
        .is_ok());
    assert!(f
        .admission()
        .execute(&registration.target, &f.b, receipt.policy_revision)
        .await
        .is_err());
    f.change(ShareGrantTransition::Revoke, "revoke-principal-share")
        .await;
    assert!(f
        .admission()
        .execute(&registration.target, &f.a, receipt.policy_revision)
        .await
        .is_err());
    let next = ShareWebhookConversation {
        registrations: &store,
        admission: f.admission(),
    }
    .execute(
        caller(&f, "synthetic-regrant"),
        f.a.clone(),
        f.target.credential.clone(),
        Capabilities::read(),
        None,
        None,
        2000,
    )
    .await
    .unwrap();
    assert!(next.policy_revision > receipt.policy_revision);
    assert!(f
        .admission()
        .execute(&registration.target, &f.a, receipt.policy_revision)
        .await
        .is_err());
    assert!(f
        .admission()
        .execute(&registration.target, &f.a, next.policy_revision)
        .await
        .is_ok());
}

struct Lookup {
    registration: Option<Registration>,
    calls: AtomicUsize,
    remove_after_first: bool,
    block_second: bool,
    substituted_recheck: Option<Registration>,
}
#[async_trait::async_trait]
impl RegistrationLookup for Lookup {
    async fn registration(&self, _: &CredentialId) -> Result<Option<Registration>, OutboxError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call > 0 && self.block_second {
            std::future::pending::<()>().await;
        }
        Ok(if call > 0 && self.remove_after_first {
            None
        } else if call > 0 && self.substituted_recheck.is_some() {
            self.substituted_recheck.clone()
        } else {
            self.registration.clone()
        })
    }
}
fn lookup(registration: Option<Registration>) -> Lookup {
    Lookup {
        registration,
        calls: AtomicUsize::new(0),
        remove_after_first: false,
        block_second: false,
        substituted_recheck: None,
    }
}
#[tokio::test]
async fn private_target_is_never_looked_up_for_another_owner() {
    let f = clean().await;
    let target = lookup(None);
    let mut other = caller(&f, "other-owner");
    other.principal_id = PrincipalId::new("someone-else").unwrap();
    let outcome = ShareWebhookConversation {
        registrations: &target,
        admission: f.admission(),
    }
    .execute(
        other,
        f.a.clone(),
        f.target.credential.clone(),
        Capabilities::read(),
        None,
        None,
        2000,
    )
    .await;
    assert!(matches!(outcome, Err(ConversationError::NotFound)));
    assert_eq!(target.calls.load(Ordering::SeqCst), 0);
    absent(&f).await;
}
#[tokio::test]
async fn a_live_auth_principal_without_a_committed_registration_is_not_a_share_target() {
    let f = clean().await;
    let missing = lookup(None);
    assert!(matches!(
        share(&f, &missing, Capabilities::read(), None).await,
        Err(ConversationError::ShareTargetNotPaired)
    ));
    assert_eq!(missing.calls.load(Ordering::SeqCst), 1);
    absent(&f).await;
}
#[tokio::test]
async fn webhook_comment_and_drive_are_refused_by_existing_cedar_before_any_grant() {
    let f = clean().await;
    let (store, _, _) = registered(&f).await;
    for (capabilities, policy) in [
        (Capabilities::comment(), None),
        (
            Capabilities::drive(),
            Some(ToolPolicy::preset(ToolPreset::All)),
        ),
    ] {
        assert!(matches!(
            share(&f, &store, capabilities, policy).await,
            Err(ConversationError::CapabilitiesRefused { .. })
        ));
        absent(&f).await;
    }
    assert!(share(&f, &store, Capabilities::read(), None).await.is_ok());
}
#[tokio::test]
async fn revoked_and_expired_credentials_cannot_create_a_principal_grant() {
    for revoked in [false, true] {
        let f = clean().await;
        let (store, _, _) = registered(&f).await;
        if revoked {
            f.credentials
                .revoke_sync(RevokeCredentialRequest {
                    request_id: "share-revoke".into(),
                    issuer_principal_id: "owner".into(),
                    credential_id: "hook-key".into(),
                    revoked_at: 3,
                })
                .unwrap();
        } else {
            f.clock.0.store(100_000, Ordering::SeqCst);
        }
        assert!(matches!(
            share(&f, &store, Capabilities::read(), None).await,
            Err(ConversationError::ShareTargetNotPaired)
        ));
        absent(&f).await;
    }
}
#[tokio::test]
async fn replaced_or_disappearing_private_registration_cannot_be_granted() {
    let f = clean().await;
    let (_, registration, _) = registered(&f).await;
    let mut target = lookup(Some(registration.clone()));
    target.remove_after_first = true;
    assert!(matches!(
        share(&f, &target, Capabilities::read(), None).await,
        Err(ConversationError::ShareTargetNotPaired)
    ));
    assert_eq!(target.calls.load(Ordering::SeqCst), 2);
    absent(&f).await;
    for substituted in 0..2 {
        let mut target = lookup(Some(registration.clone()));
        let mut changed = registration.clone();
        if substituted == 0 {
            changed.id = Uuid::new_v4();
        } else {
            changed.target.principal = PrincipalId::new("substituted-principal").unwrap();
        }
        target.substituted_recheck = Some(changed);
        assert!(matches!(
            share(&f, &target, Capabilities::read(), None).await,
            Err(ConversationError::ShareTargetNotPaired)
        ));
        absent(&f).await;
    }
    for field in 0..4 {
        let mut incorrect = registration.clone();
        match field {
            0 => incorrect.target.credential = CredentialId::new("different-key").unwrap(),
            1 => incorrect.target.principal = PrincipalId::new("different-principal").unwrap(),
            2 => incorrect.target.organization = OrganizationId::new("different-org").unwrap(),
            _ => incorrect.target.owner = PrincipalId::new("different-owner").unwrap(),
        }
        assert!(matches!(
            share(&f, &lookup(Some(incorrect)), Capabilities::read(), None).await,
            Err(ConversationError::ShareTargetNotPaired)
        ));
        absent(&f).await;
    }
}
#[tokio::test]
async fn cancelling_share_during_private_recheck_releases_original_auth_without_grant() {
    let f = clean().await;
    let (_, registration, _) = registered(&f).await;
    let mut target = lookup(Some(registration));
    target.block_second = true;
    let mut pending = Box::pin(share(&f, &target, Capabilities::read(), None));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::select! {
            result = pending.as_mut() => panic!("share completed before its private gate: {result:?}"),
            _ = async { while target.calls.load(Ordering::SeqCst)<2 { tokio::task::yield_now().await; } } => {}
        }
    }).await.expect("share entered private recheck");
    assert_eq!(target.calls.load(Ordering::SeqCst), 2);
    let mut revoke = Box::pin(f.credentials.revoke(RevokeCredentialRequest {
        request_id: "cancelled-share-revoke".into(),
        issuer_principal_id: "owner".into(),
        credential_id: "hook-key".into(),
        revoked_at: 3,
    }));
    assert!(matches!(
        revoke
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    drop(pending);
    revoke.await.unwrap();
    absent(&f).await;
}

struct ReleaseWrite(Arc<GrantWritePause>);
impl Drop for ReleaseWrite {
    fn drop(&mut self) {
        self.0.release();
    }
}
#[tokio::test]
async fn cancelled_physical_share_write_retains_original_auth_until_actual_commit() {
    let f = Arc::new(clean().await);
    let (store, registration, _) = registered(&f).await;
    let store = Arc::new(store);
    let pause = GrantWritePause::new();
    let _release = ReleaseWrite(pause.clone());
    f.grants.pause_grant_writes(pause.clone());
    let sharing = {
        let f = f.clone();
        let store = store.clone();
        tokio::spawn(async move { share(&f, &*store, Capabilities::read(), None).await })
    };
    let entered = {
        let pause = pause.clone();
        tokio::task::spawn_blocking(move || pause.wait_until_entered())
    };
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("real grant write entered")
        .unwrap();
    sharing.abort();
    assert!(sharing.await.unwrap_err().is_cancelled());
    let mut revoking = {
        let f = f.clone();
        tokio::spawn(async move {
            f.credentials
                .revoke(RevokeCredentialRequest {
                    request_id: "physical-share-revoke".into(),
                    issuer_principal_id: "owner".into(),
                    credential_id: "hook-key".into(),
                    revoked_at: 3,
                })
                .await
        })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut revoking)
            .await
            .is_err(),
        "revocation cannot pass the original Auth hold while the physical grant can still commit"
    );
    pause.release();
    tokio::time::timeout(Duration::from_secs(5), revoking)
        .await
        .expect("revocation joins after actual grant write")
        .unwrap()
        .unwrap();
    let grant = f
        .grants
        .principal_grant(&f.a, &f.target.principal)
        .await
        .unwrap()
        .unwrap();
    assert!(grant.receiver_id.is_none());
    assert!(
        f.admission()
            .execute(&registration.target, &f.a, grant.policy_revision)
            .await
            .is_err(),
        "committed history grants no access after actual revocation"
    );
}
struct ExpiringLookup<'a> {
    registration: Registration,
    clock: &'a Time,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl RegistrationLookup for ExpiringLookup<'_> {
    async fn registration(&self, _: &CredentialId) -> Result<Option<Registration>, OutboxError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
            self.clock.0.store(100_000, Ordering::SeqCst);
        }
        Ok(Some(self.registration.clone()))
    }
}
#[tokio::test]
async fn expiry_during_private_registration_recheck_refuses_before_grant() {
    let f = clean().await;
    let (_, registration, _) = registered(&f).await;
    let target = ExpiringLookup {
        registration,
        clock: &f.clock,
        calls: AtomicUsize::new(0),
    };
    assert!(matches!(
        share(&f, &target, Capabilities::read(), None).await,
        Err(ConversationError::ShareTargetNotPaired)
    ));
    assert_eq!(target.calls.load(Ordering::SeqCst), 2);
    absent(&f).await;
}

#[tokio::test]
async fn credential_owning_grant_port_refuses_mismatched_credential_and_principal() {
    let f = clean().await;
    for wrong_credential in [false, true] {
        let held = f.credentials.hold(&f.target.credential).await.unwrap();
        let result = f
            .grants
            .change_with_credential(
                ShareGrantChange {
                    transition: ShareGrantTransition::Grant(ShareAccess::read()),
                    conversation_id: f.a.clone(),
                    principal_id: Some(if wrong_credential {
                        f.target.principal.clone()
                    } else {
                        PrincipalId::new("unrelated-principal").unwrap()
                    }),
                    receiver_id: None,
                    credential_id: if wrong_credential {
                        CredentialId::new("unrelated-credential").unwrap()
                    } else {
                        f.target.credential.clone()
                    },
                    expected_policy_revision: None,
                    initiator: caller(&f, "mismatched-held-target"),
                    at_ms: 2000,
                },
                held,
            )
            .await;
        assert!(matches!(result, Err(ConversationError::Metadata)));
        absent(&f).await;
    }
}

struct PanicGrantPolicy(AtomicUsize);
impl ConversationAccessPolicyEvaluator for PanicGrantPolicy {
    fn evaluate(&self, _: &AccessAsk<'_>) -> AccessDecision {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("synthetic physical grant policy panic");
    }
}
#[tokio::test]
async fn physical_grant_panic_rolls_back_and_releases_original_auth_after_worker_exits() {
    let mut f = clean().await;
    let (store, _, _) = registered(&f).await;
    let panicking = Arc::new(PanicGrantPolicy(AtomicUsize::new(0)));
    f.grants = f.grants.with_access_policy(panicking.clone());
    assert!(matches!(
        share(&f, &store, Capabilities::read(), None).await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(panicking.0.load(Ordering::SeqCst), 1);
    tokio::time::timeout(
        Duration::from_secs(5),
        f.credentials.revoke(RevokeCredentialRequest {
            request_id: "physical-grant-panic-revoke".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "hook-key".into(),
            revoked_at: 3,
        }),
    )
    .await
    .expect("physical unwind released the original Auth hold")
    .unwrap();
    absent(&f).await;
}
