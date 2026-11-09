//! Real enrollment, registry, Cedar and grant-file boundaries for #893.
use super::support::{pending, private_root, Fixture, WAIT};
use nessa_auth::{
    adapters::pairing::{FilePairingState, ManualCode, OsEntropy},
    application::{
        credential_admin::RevokeCredentialRequest,
        pairing::{
            ClientPendingStore, DeviceCredential, PairingStore, PendingEnrollment,
            PrivateKeyMaterial, PrivateStateError,
        },
    },
    domain::{
        pairing::{peer_principal, ConsentClass, DeviceKey, InvitationId, PairingPhase},
        CredentialId, ResourceId,
    },
};
use nessa_client_core::pairing::NativeEnrollmentClient;
use nessa_local_database::rusqlite::Connection;
use nessa_protocol::{
    agents::AgentId,
    conversation::domain::{ConversationApprovalMode, ConversationId, ConversationModelId},
    pairing::wire::NativePairingStatus,
};
use nessa_server::{
    app::dependencies::RuntimeDependencies,
    conversation::{
        application::{
            AccessAction, ConversationCaller, ConversationRepository, PairedAccess, PairedParty,
            PairedRefusal, RequestShape, ShareGrantChange, ShareGrantTransition, ShareGrants,
        },
        domain::{Conversation, ShareAccess},
        infrastructure::{CedarConversationAccessPolicy, LocalConversationStore},
    },
};
use std::{
    net::TcpStream,
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

/// Synthetic peer keys stay stable across re-enrollment, as a real gateway's do.
struct KeyedStore {
    inner: Arc<FilePairingState>,
    seed: u8,
}
impl ClientPendingStore for KeyedStore {
    fn enrollment_key(&self) -> Result<Option<PrivateKeyMaterial>, PrivateStateError> {
        Ok(Some(PrivateKeyMaterial::new(Zeroizing::new(
            [self.seed; 32],
        ))))
    }
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.inner.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.inner.load_credential()
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        expected: nessa_auth::domain::pairing::PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.inner.save_credential(credential, receiver, expected)
    }
    fn end_enrollment(
        &self,
        expected: nessa_auth::domain::pairing::PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.inner.end_enrollment(expected)
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        pin: &[u8; 44],
        intent: nessa_auth::domain::pairing::PublicIntent,
        expected: Option<nessa_auth::domain::pairing::PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.inner.save_pending(key, pin, intent, expected)
    }
}
async fn claim(fixture: &Fixture, name: &str, choice: bool, seed: u8) -> (InvitationId, DeviceKey) {
    claim_class(fixture, name, ConsentClass::PeerRead, choice, seed).await
}
async fn claim_class(
    fixture: &Fixture,
    name: &str,
    class: ConsentClass,
    choice: bool,
    seed: u8,
) -> (InvitationId, DeviceKey) {
    let created = fixture
        .gateway
        .create(fixture.session.clone(), class, choice, OsEntropy)
        .await
        .unwrap();
    assert_eq!(created.record().intent().same_person(), choice);
    let (address, stop, listener, _) = fixture.listener().await;
    let (_, inner) = pending(fixture.directory.path(), name);
    let store = Arc::new(KeyedStore { inner, seed });
    let client = NativeEnrollmentClient::enrolling(
        class,
        store.clone(),
        RuntimeDependencies::default().clock,
    );
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let status = tokio::time::timeout(
        WAIT,
        client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(status, NativePairingStatus::Claimed(_)));
    assert_eq!(status.public().same_person(), choice);
    assert_eq!(
        store
            .load_pending()
            .unwrap()
            .unwrap()
            .intent()
            .same_person(),
        choice
    );
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    let id = created.record().id();
    let (_, key) = fixture
        .registry
        .read_pairing(id)
        .unwrap()
        .claim_binding()
        .unwrap();
    (id, key)
}
fn grant_store(fixture: &Fixture) -> (Arc<LocalConversationStore>, PathBuf) {
    let root = private_root(fixture.directory.path(), "person-link-grants");
    let path = root.join("metadata.sqlite3");
    let store = Arc::new(
        LocalConversationStore::open(&path)
            .unwrap()
            .with_access_policy(Arc::new(CedarConversationAccessPolicy::new().unwrap())),
    );
    let links = store
        .bind_person_link_source(fixture.registry.clone(), fixture.time.clone())
        .unwrap();
    fixture.registry.bind_person_links(links).unwrap();
    (store, path.with_extension("grants.sqlite3"))
}
fn raw(path: &Path) -> Connection {
    Connection::open(path).unwrap()
}
fn link_count(path: &Path) -> i64 {
    raw(path)
        .query_row("SELECT count(*) FROM person_links", [], |row| row.get(0))
        .unwrap()
}
fn audit_count(path: &Path) -> i64 {
    raw(path)
        .query_row("SELECT count(*) FROM person_link_changes", [], |row| {
            row.get(0)
        })
        .unwrap()
}
fn chat(fixture: &Fixture) -> Conversation {
    Conversation::new(
        ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap(),
        fixture.session.context().organization_id().clone(),
        fixture.session.context().principal_id().clone(),
        "panel".into(),
        "create".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap()
}
fn share(
    fixture: &Fixture,
    conversation: &Conversation,
    record: &nessa_auth::domain::pairing::PairingRecord,
    request: &str,
) -> ShareGrantChange {
    ShareGrantChange {
        transition: ShareGrantTransition::Grant(ShareAccess::read()),
        conversation_id: conversation.id().clone(),
        principal_id: record.credential_principal(),
        receiver_id: Some(record.receiver_binding().unwrap().0.as_str().to_owned()),
        credential_id: record.credential().unwrap().clone(),
        expected_policy_revision: None,
        initiator: ConversationCaller {
            organization_id: fixture.session.context().organization_id().clone(),
            principal_id: fixture.session.context().principal_id().clone(),
            surface_id: "test".into(),
            action_id: request.into(),
        },
        at_ms: 110_001,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linked_gateways_keep_distinct_grants_and_a_plain_gateway_reports_no_person() {
    let fixture = Fixture::new().await;
    let (metadata, path) = grant_store(&fixture);
    let conversation = chat(&fixture);
    metadata.create(conversation.clone()).await.unwrap();
    let policy = CedarConversationAccessPolicy::new().unwrap();
    let mut records = Vec::new();
    for (name, choice, seed) in [
        ("first", true, 41),
        ("second", true, 42),
        ("plain", false, 43),
    ] {
        let (id, key) = claim(&fixture, name, choice, seed).await;
        let approval = fixture
            .gateway
            .approve(&fixture.session, id, key, OsEntropy)
            .await
            .unwrap();
        assert!(approval.stopped.is_none());
        let record = approval.record;
        assert_eq!(record.phase(), PairingPhase::Active);
        let principal = peer_principal(&key).unwrap();
        assert_eq!(record.credential_principal(), Some(principal.clone()));
        assert_eq!(record.intent().class(), ConsentClass::PeerRead);
        let receiver = record.receiver_binding().unwrap().0.as_str();
        let party = PairedParty {
            organization: conversation.organization(),
            signed_in_as: &principal,
            binding_owner: conversation.owner(),
            receiver,
        };
        let access = PairedAccess::new(&policy, party);
        assert_eq!(
            metadata
                .held_grant(conversation.id(), receiver)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            access.admit(
                AccessAction::Read,
                &conversation,
                None,
                RequestShape::default()
            ),
            Err(PairedRefusal::NotFound)
        );
        assert!(
            metadata
                .change(share(&fixture, &conversation, &record, name))
                .await
                .unwrap()
                .applied
        );
        let held = metadata
            .held_grant(conversation.id(), receiver)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.principal_id, principal);
        assert_eq!(held.person.as_ref(), choice.then_some(conversation.owner()));
        access
            .admit(
                AccessAction::Read,
                &conversation,
                Some(held.capabilities),
                RequestShape::default(),
            )
            .unwrap();
        for action in [AccessAction::Comment, AccessAction::Drive] {
            assert_eq!(
                access.admit(
                    action,
                    &conversation,
                    Some(held.capabilities),
                    RequestShape::default()
                ),
                Err(PairedRefusal::Forbidden)
            );
        }
        records.push(record);
    }
    assert_eq!(link_count(&path), 2);
    assert_eq!(audit_count(&path), 2);
    let before = metadata.grants(conversation.id()).await.unwrap();
    assert_eq!(before.grants.len(), 3);
    fixture
        .registry
        .revoke_sync(RevokeCredentialRequest {
            request_id: "revoke-first".into(),
            issuer_principal_id: "owner".into(),
            credential_id: records[0].credential().unwrap().as_str().into(),
            revoked_at: 111,
        })
        .unwrap();
    assert_eq!(link_count(&path), 1);
    let after = metadata.grants(conversation.id()).await.unwrap();
    assert_eq!(
        after, before,
        "credential revoke removes attribution, not grants or epochs"
    );
    let receiver = records[0].receiver_binding().unwrap().0.as_str();
    assert_eq!(
        metadata
            .held_grant(conversation.id(), receiver)
            .await
            .unwrap()
            .unwrap()
            .person,
        None
    );
    let cause:(String,String,String)=raw(&path).query_row("SELECT cause,initiator_kind,initiator_id FROM person_link_changes ORDER BY sequence DESC LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(
        cause,
        (
            "credential_revoked".into(),
            "principal".into(),
            "owner".into()
        )
    );
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_link_write_leaves_active_and_next_grant_write_commits_attribution_first() {
    for table in ["person_links", "person_link_changes"] {
        let fixture = Fixture::new().await;
        let (metadata, path) = grant_store(&fixture);
        let conversation = chat(&fixture);
        metadata.create(conversation.clone()).await.unwrap();
        raw(&path).execute_batch(&format!("CREATE TRIGGER fail_link BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'unavailable'); END;")).unwrap();
        let (id, key) = claim(&fixture, "retry", true, 44).await;
        let approval = fixture
            .gateway
            .approve(&fixture.session, id, key, OsEntropy)
            .await
            .unwrap();
        assert_eq!(approval.record.phase(), PairingPhase::Active);
        assert!(approval.stopped.is_some());
        assert_eq!(
            fixture.registry.read_pairing(id).unwrap().phase(),
            PairingPhase::Active
        );
        assert_eq!(link_count(&path), 0);
        let retry = fixture
            .gateway
            .approve(&fixture.session, id, key, OsEntropy)
            .await
            .unwrap();
        assert_eq!(retry.record.phase(), PairingPhase::Active);
        assert!(
            retry.stopped.is_some(),
            "an owed link is not reported finished by an Active receipt"
        );
        assert!(metadata
            .change(share(
                &fixture,
                &conversation,
                &approval.record,
                "while-failed"
            ))
            .await
            .is_err());
        assert!(metadata
            .grants(conversation.id())
            .await
            .unwrap()
            .grants
            .is_empty());
        raw(&path).execute_batch("DROP TRIGGER fail_link").unwrap();
        if table == "person_link_changes" {
            let retried = fixture
                .gateway
                .approve(&fixture.session, id, key, OsEntropy)
                .await
                .unwrap();
            assert_eq!(retried.record.credential(), approval.record.credential());
            assert_eq!(
                retried.record.receiver_binding(),
                approval.record.receiver_binding()
            );
            assert!(retried.stopped.is_none());
            assert_eq!(link_count(&path), 1);
        }
        assert!(
            metadata
                .change(share(&fixture, &conversation, &approval.record, "restored"))
                .await
                .unwrap()
                .applied
        );
        let held = metadata
            .held_grant(
                conversation.id(),
                approval.record.receiver_binding().unwrap().0.as_str(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.person.as_ref(), Some(conversation.owner()));
        assert_eq!(link_count(&path), 1);
        assert_eq!(audit_count(&path), 1);
        metadata.grants(conversation.id()).await.unwrap();
        assert_eq!(
            audit_count(&path),
            1,
            "retries do not duplicate the original choice audit"
        );
        fixture.gateway.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_preserves_claimed_choice_and_repairs_active_link_gap() {
    let fixture = Fixture::new().await;
    let (id, key) = claim(&fixture, "restart", true, 45).await;
    fixture.gateway.shutdown().await;
    let fixture = fixture.reopen().await;
    let restored = fixture.registry.read_pairing(id).unwrap();
    assert_eq!(restored.phase(), PairingPhase::Claimed);
    assert!(restored.intent().same_person());
    let (metadata, path) = grant_store(&fixture);
    raw(&path).execute_batch("CREATE TRIGGER fail_link BEFORE INSERT ON person_links BEGIN SELECT RAISE(ABORT,'unavailable'); END;").unwrap();
    let approval = fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap();
    assert_eq!(approval.record.phase(), PairingPhase::Active);
    assert!(approval.stopped.is_some());
    assert_eq!(link_count(&path), 0);
    fixture.gateway.shutdown().await;
    drop(metadata);
    raw(&path).execute_batch("DROP TRIGGER fail_link").unwrap();
    let fixture = fixture.reopen().await;
    assert_eq!(
        link_count(&path),
        0,
        "a credential restart alone is not a grant commit"
    );
    let (_metadata, path) = grant_store(&fixture);
    assert_eq!(
        fixture.registry.read_pairing(id).unwrap().phase(),
        PairingPhase::Active
    );
    assert_eq!(link_count(&path), 1);
    assert_eq!(audit_count(&path), 1);
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_unlink_blocks_same_key_activation_and_a_false_choice_inherits_nothing() {
    let fixture = Fixture::new().await;
    let (_metadata, path) = grant_store(&fixture);
    let (id, key) = claim(&fixture, "old", true, 46).await;
    let old = fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap();
    assert!(old.stopped.is_none());
    raw(&path).execute_batch("CREATE TRIGGER fail_unlink BEFORE DELETE ON person_links BEGIN SELECT RAISE(ABORT,'unavailable'); END;").unwrap();
    let revoked = fixture.registry.revoke_sync(RevokeCredentialRequest {
        request_id: "revoke-old".into(),
        issuer_principal_id: "owner".into(),
        credential_id: old.record.credential().unwrap().as_str().into(),
        revoked_at: 111,
    });
    assert!(matches!(
        revoked,
        Err(nessa_auth::adapters::local::LocalStoreError::PersonLinksUnavailable)
    ));
    assert_eq!(
        fixture.registry.read_pairing(id).unwrap().phase(),
        PairingPhase::Terminal
    );
    assert_eq!(link_count(&path), 1);
    let (new_id, new_key) = claim(&fixture, "new", false, 46).await;
    assert_eq!(key, new_key);
    let blocked = fixture
        .gateway
        .approve(&fixture.session, new_id, new_key, OsEntropy)
        .await
        .unwrap();
    assert_eq!(blocked.record.phase(), PairingPhase::Staging);
    assert!(blocked.stopped.is_some());
    assert_eq!(link_count(&path), 1);
    raw(&path)
        .execute_batch("DROP TRIGGER fail_unlink")
        .unwrap();
    let activated = fixture
        .gateway
        .approve(&fixture.session, new_id, new_key, OsEntropy)
        .await
        .unwrap();
    assert_eq!(activated.record.phase(), PairingPhase::Active);
    assert!(activated.stopped.is_none());
    assert!(!activated.record.intent().same_person());
    assert_eq!(link_count(&path), 0);
    assert_eq!(audit_count(&path), 2);
    let actor: (String, String) = raw(&path)
        .query_row(
            "SELECT cause,initiator_id FROM person_link_changes ORDER BY sequence DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(actor, ("credential_revoked".into(), "owner".into()));
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enrollment_revoke_commits_phase_before_unlink_and_retry_keeps_owner_cause() {
    let fixture = Fixture::new().await;
    let (metadata, path) = grant_store(&fixture);
    let conversation = chat(&fixture);
    metadata.create(conversation.clone()).await.unwrap();
    let (id, key) = claim(&fixture, "cancel", true, 47).await;
    let old = fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap();
    assert!(old.stopped.is_none());
    assert!(
        metadata
            .change(share(&fixture, &conversation, &old.record, "share"))
            .await
            .unwrap()
            .applied
    );
    let before = metadata.grants(conversation.id()).await.unwrap();
    raw(&path).execute_batch("CREATE TRIGGER fail_unlink BEFORE DELETE ON person_links BEGIN SELECT RAISE(ABORT,'unavailable'); END;").unwrap();
    assert!(fixture
        .gateway
        .decide(
            &fixture.session,
            id,
            nessa_auth::application::pairing::OwnerDecision::Cancel
        )
        .await
        .is_err());
    assert_eq!(
        fixture.registry.read_pairing(id).unwrap().phase(),
        PairingPhase::Terminal
    );
    assert_eq!(link_count(&path), 1);
    raw(&path)
        .execute_batch("DROP TRIGGER fail_unlink")
        .unwrap();
    assert_eq!(metadata.grants(conversation.id()).await.unwrap(), before);
    assert_eq!(link_count(&path), 0);
    let actor: (String, String) = raw(&path)
        .query_row(
            "SELECT cause,initiator_id FROM person_link_changes ORDER BY sequence DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(actor, ("enrollment_revoked".into(), "owner".into()));
    metadata.grants(conversation.id()).await.unwrap();
    assert_eq!(audit_count(&path), 2);
    fixture.gateway.shutdown().await;
}

/// An injected credential authority with a finite peer lifetime. The local
/// enrollment issuer uses no expiry, so this exercises the port's expiry case.
struct FiniteSource(Vec<nessa_auth::application::pairing::PairingLinkEvidence>);
impl nessa_auth::application::pairing::PairingLinkSource for FiniteSource {
    fn with_current_links(
        &self,
        effect: &mut dyn FnMut(
            &[nessa_auth::application::pairing::PairingLinkEvidence],
        )
            -> Result<(), nessa_auth::application::pairing::PersonLinkError>,
    ) -> Result<(), nessa_auth::application::pairing::PersonLinkError> {
        effect(&self.0)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expiry_removal_is_durable_and_invalid_combined_evidence_has_no_partial_effect() {
    use nessa_auth::application::pairing::{PairingLinkSource, PersonLinkError};
    let fixture = Fixture::new().await;
    let (_actual_metadata, _actual_path) = grant_store(&fixture);
    let (id, key) = claim(&fixture, "finite", true, 48).await;
    let record = fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap()
        .record;
    let mut facts = None;
    fixture
        .registry
        .with_current_links(&mut |current| {
            facts = Some(current.to_vec());
            Ok(())
        })
        .unwrap();
    let mut facts = facts.unwrap();
    let original = &facts[0].credential;
    facts[0].credential = nessa_auth::domain::Credential::new(
        original.id().clone(),
        original.principal_id().clone(),
        original.organization_id().clone(),
        original.audience_id().clone(),
        original.issued_at(),
        Some(111),
        original.grants().to_vec(),
    )
    .unwrap();
    let source = Arc::new(FiniteSource(facts));
    let root = private_root(fixture.directory.path(), "finite-grants");
    let metadata_path = root.join("metadata.sqlite3");
    let path = metadata_path.with_extension("grants.sqlite3");
    let metadata = LocalConversationStore::open(&metadata_path)
        .unwrap()
        .with_access_policy(Arc::new(CedarConversationAccessPolicy::new().unwrap()));
    let links = metadata
        .bind_person_link_source(source.clone(), fixture.time.clone())
        .unwrap();
    source
        .with_current_links(&mut |facts| {
            let mut invalid = facts[0].clone();
            let credential = &invalid.credential;
            invalid.credential = nessa_auth::domain::Credential::new(
                CredentialId::new("another-credential").unwrap(),
                credential.principal_id().clone(),
                credential.organization_id().clone(),
                credential.audience_id().clone(),
                credential.issued_at(),
                credential.expires_at(),
                credential.grants().to_vec(),
            )
            .unwrap();
            assert_eq!(
                links.reconcile(&[facts[0].clone(), invalid]),
                Err(PersonLinkError::Conflict)
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(link_count(&path), 0);
    assert_eq!(audit_count(&path), 0);
    let conversation = chat(&fixture);
    metadata.create(conversation.clone()).await.unwrap();
    assert!(
        metadata
            .change(share(&fixture, &conversation, &record, "finite-share"))
            .await
            .unwrap()
            .applied
    );
    let receiver = record.receiver_binding().unwrap().0.as_str();
    assert_eq!(
        metadata
            .held_grant(conversation.id(), receiver)
            .await
            .unwrap()
            .unwrap()
            .person
            .as_ref(),
        Some(conversation.owner())
    );
    fixture.time.set(111_000);
    raw(&path).execute_batch("CREATE TRIGGER fail_expiry_audit BEFORE INSERT ON person_link_changes BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;").unwrap();
    assert_eq!(
        links.reconcile(&source.0),
        Err(PersonLinkError::Unavailable)
    );
    assert_eq!(
        link_count(&path),
        1,
        "audit failure rolls the removal back and leaves the obligation"
    );
    raw(&path)
        .execute_batch("DROP TRIGGER fail_expiry_audit")
        .unwrap();
    assert_eq!(
        metadata
            .held_grant(conversation.id(), receiver)
            .await
            .unwrap()
            .unwrap()
            .person,
        None
    );
    assert_eq!(link_count(&path), 0);
    let cause:(String,String,Option<String>)=raw(&path).query_row("SELECT cause,initiator_kind,initiator_id FROM person_link_changes ORDER BY sequence DESC LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(cause, ("credential_expired".into(), "expiry".into(), None));
    fixture.time.set(110_001);
    assert_eq!(
        metadata
            .held_grant(conversation.id(), receiver)
            .await
            .unwrap()
            .unwrap()
            .person,
        None,
        "a committed removal consumes this invitation's choice"
    );
    assert_eq!(audit_count(&path), 2);
    drop(links);
    drop(metadata);
    let metadata = LocalConversationStore::open(&metadata_path).unwrap();
    let _links = metadata
        .bind_person_link_source(source.clone(), fixture.time.clone())
        .unwrap();
    assert_eq!(
        metadata
            .held_grant(conversation.id(), receiver)
            .await
            .unwrap()
            .unwrap()
            .person,
        None
    );
    assert_eq!(audit_count(&path), 2);
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_with_no_agent_or_conversation_store_can_commit_the_link() {
    let fixture = Fixture::new().await;
    let root = private_root(fixture.directory.path(), "grant-only");
    let path = root.join("metadata.grants.sqlite3");
    let metadata = root.join("metadata.sqlite3");
    let links = nessa_server::conversation::infrastructure::LocalPersonLinks::open(
        &path,
        fixture.registry.clone(),
        fixture.time.clone(),
    )
    .unwrap();
    fixture.registry.bind_person_links(links).unwrap();
    assert!(!metadata.exists());
    let (id, key) = claim(&fixture, "grant-only-peer", true, 49).await;
    let approval = fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap();
    assert_eq!(approval.record.phase(), PairingPhase::Active);
    assert!(approval.stopped.is_none());
    assert_eq!(link_count(&path), 1);
    assert_eq!(audit_count(&path), 1);
    assert!(
        !metadata.exists(),
        "the grant owner does not create or open a conversation store"
    );
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owed_gateway_link_does_not_interrupt_device_or_owner_phone_activation() {
    let fixture = Fixture::phone_owner().await;
    let (_metadata, path) = grant_store(&fixture);
    raw(&path).execute_batch("CREATE TRIGGER fail_link BEFORE INSERT ON person_links BEGIN SELECT RAISE(ABORT,'unavailable'); END;").unwrap();
    let (id, key) = claim(&fixture, "owed", true, 71).await;
    let peer = fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap();
    assert_eq!(peer.record.phase(), PairingPhase::Active);
    assert!(peer.stopped.is_some());
    for (class, name, seed) in [
        (ConsentClass::DeviceRead, "device", 72),
        (ConsentClass::OwnerPhoneReadWrite, "phone", 73),
    ] {
        let (id, key) = claim_class(&fixture, name, class, false, seed).await;
        let approval = fixture
            .gateway
            .approve(&fixture.session, id, key, OsEntropy)
            .await
            .unwrap();
        assert_eq!(approval.record.phase(), PairingPhase::Active);
        assert!(approval.stopped.is_none());
        assert_eq!(approval.record.intent().class(), class);
        assert_eq!(
            approval.record.credential_principal().as_ref(),
            Some(approval.record.intent().owner())
        );
        assert!(!approval.record.intent().same_person());
    }
    assert_eq!(link_count(&path), 0);
    assert_eq!(audit_count(&path), 0);
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attribution_requires_the_exact_granted_credential_and_organization() {
    let fixture = Fixture::new().await;
    // Leave the effect source unbound so this adapter test can retain a stale
    // physical row and prove admission's independent credential/organization join.
    let root = private_root(fixture.directory.path(), "unbound-grants");
    let path = root.join("metadata.sqlite3");
    let metadata = LocalConversationStore::open(&path)
        .unwrap()
        .with_access_policy(Arc::new(CedarConversationAccessPolicy::new().unwrap()));
    let conversation = chat(&fixture);
    metadata.create(conversation.clone()).await.unwrap();
    let (id, key) = claim(&fixture, "plain", false, 74).await;
    let approval = fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap();
    assert!(approval.stopped.is_none());
    assert!(
        metadata
            .change(share(&fixture, &conversation, &approval.record, "share"))
            .await
            .unwrap()
            .applied
    );
    let receiver = approval.record.receiver_binding().unwrap().0.as_str();
    let before = metadata
        .held_grant(conversation.id(), receiver)
        .await
        .unwrap()
        .unwrap();
    assert!(before.person.is_none());
    let connection = raw(&path.with_extension("grants.sqlite3"));
    let organization = conversation.organization().as_str();
    let credential = approval.record.credential().unwrap().as_str();
    for (org, credential, linked) in [
        (organization, "earlier-credential", false),
        ("another-organization", credential, false),
        (organization, credential, true),
    ] {
        connection.execute("DELETE FROM person_links", []).unwrap();
        connection.execute(
            "INSERT INTO person_links (organization_id,gateway_principal,person_principal,credential_id,invitation) VALUES (?1,?2,'person',?3,'synthetic-invitation')",
            nessa_local_database::rusqlite::params![org, before.principal_id.as_str(), credential],
        ).unwrap();
        let held = metadata
            .held_grant(conversation.id(), receiver)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.principal_id, before.principal_id);
        assert_eq!(held.policy_revision, before.policy_revision);
        assert_eq!(held.capabilities, before.capabilities);
        assert_eq!(
            held.person,
            linked.then(|| nessa_auth::domain::PrincipalId::new("person").unwrap())
        );
    }
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_devices_of_one_owner_keep_independent_receiver_grants() {
    let fixture = Fixture::new().await;
    let (metadata, _path) = grant_store(&fixture);
    let conversation = chat(&fixture);
    metadata.create(conversation.clone()).await.unwrap();
    let mut records = Vec::new();
    for (name, seed) in [("first-device", 75), ("second-device", 76)] {
        let (id, key) = claim_class(&fixture, name, ConsentClass::DeviceRead, false, seed).await;
        let approval = fixture
            .gateway
            .approve(&fixture.session, id, key, OsEntropy)
            .await
            .unwrap();
        assert!(approval.stopped.is_none());
        assert_eq!(
            approval.record.credential_principal().as_ref(),
            Some(conversation.owner())
        );
        assert!(
            metadata
                .change(share(&fixture, &conversation, &approval.record, name))
                .await
                .unwrap()
                .applied
        );
        records.push(approval.record);
    }
    let first = records[0].receiver_binding().unwrap().0.as_str();
    let second = records[1].receiver_binding().unwrap().0.as_str();
    assert_ne!(first, second);
    let grants = metadata.grants(conversation.id()).await.unwrap();
    assert_eq!(grants.grants.len(), 2);
    assert!(grants
        .grants
        .iter()
        .all(|grant| &grant.principal_id == conversation.owner()));
    let retained = metadata
        .held_grant(conversation.id(), second)
        .await
        .unwrap()
        .unwrap();
    assert!(retained.person.is_none());
    let mut revoke = share(&fixture, &conversation, &records[0], "revoke-first");
    revoke.transition = ShareGrantTransition::Revoke;
    assert!(metadata.change(revoke).await.unwrap().applied);
    assert_eq!(
        metadata.held_grant(conversation.id(), first).await.unwrap(),
        None
    );
    assert_eq!(
        metadata
            .held_grant(conversation.id(), second)
            .await
            .unwrap(),
        Some(retained)
    );
    assert_eq!(
        metadata
            .grants(conversation.id())
            .await
            .unwrap()
            .grants
            .len(),
        1
    );
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expiry_before_the_first_link_consumes_the_choice_without_replay() {
    use nessa_auth::application::pairing::{
        PairingLinkSource, PairingPersonLinks, PersonLinkError,
    };
    use nessa_server::conversation::infrastructure::LocalPersonLinks;
    let fixture = Fixture::new().await;
    let (_metadata, _) = grant_store(&fixture);
    let (id, key) = claim(&fixture, "expired-before-link", true, 52).await;
    fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap();
    let mut facts = Vec::new();
    fixture
        .registry
        .with_current_links(&mut |current| {
            facts = current.to_vec();
            Ok(())
        })
        .unwrap();
    let original = &facts[0].credential;
    facts[0].credential = nessa_auth::domain::Credential::new(
        original.id().clone(),
        original.principal_id().clone(),
        original.organization_id().clone(),
        original.audience_id().clone(),
        original.issued_at(),
        Some(111),
        original.grants().to_vec(),
    )
    .unwrap();
    let source = Arc::new(FiniteSource(facts));
    let root = private_root(fixture.directory.path(), "expired-unlinked");
    let path = root.join("grants.sqlite3");
    fixture.time.set(111_000);
    let links = LocalPersonLinks::open(&path, source.clone(), fixture.time.clone()).unwrap();
    raw(&path).execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON person_link_changes BEGIN SELECT RAISE(ABORT,'unavailable'); END;").unwrap();
    assert_eq!(
        links.reconcile(&source.0),
        Err(PersonLinkError::Unavailable)
    );
    assert_eq!(link_count(&path), 0);
    assert_eq!(audit_count(&path), 0);
    raw(&path)
        .execute_batch("DROP TRIGGER fail_receipt")
        .unwrap();
    links.reconcile(&source.0).unwrap();
    assert_eq!(link_count(&path), 0);
    assert_eq!(audit_count(&path), 1);
    let removed: (Option<String>, Option<String>, String, String) = raw(&path)
        .query_row(
            "SELECT person_before,person_after,cause,initiator_kind FROM person_link_changes",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        removed,
        (None, None, "credential_expired".into(), "expiry".into())
    );
    drop(links);
    fixture.time.set(110_001);
    let links = LocalPersonLinks::open(&path, source.clone(), fixture.time.clone()).unwrap();
    links.reconcile(&source.0).unwrap();
    assert_eq!(
        link_count(&path),
        0,
        "a choice consumed before insertion stays consumed after restart and clock rollback"
    );
    assert_eq!(audit_count(&path), 1);
    // An old source cut can also meet a newer credential's committed link.
    // Its receipt must describe no removal, and leave that link byte-for-byte.
    fixture.time.set(111_000);
    let later_path = root.join("later.sqlite3");
    let links = LocalPersonLinks::open(&later_path, source.clone(), fixture.time.clone()).unwrap();
    let credential = &source.0[0].credential;
    raw(&later_path).execute("INSERT INTO person_links VALUES (?1,?2,'new-person','new-credential','new-invitation')",
        [credential.organization_id().as_str(),credential.principal_id().as_str()]).unwrap();
    links.reconcile(&source.0).unwrap();
    let later: (String, String, String) = raw(&later_path)
        .query_row(
            "SELECT person_principal,credential_id,invitation FROM person_links",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        later,
        (
            "new-person".into(),
            "new-credential".into(),
            "new-invitation".into()
        )
    );
    let receipt: (String, Option<String>, Option<String>) = raw(&later_path)
        .query_row(
            "SELECT credential_id,person_before,person_after FROM person_link_changes",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(receipt, (credential.id().as_str().to_owned(), None, None));
    links.reconcile(&source.0).unwrap();
    assert_eq!(audit_count(&later_path), 1);
    assert_eq!(link_count(&later_path), 1);
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_terminal_evidence_is_correlated_before_any_link_or_audit_effect() {
    use nessa_auth::application::pairing::{PairingLinkSource, PersonLinkError};
    use nessa_auth::domain::pairing::{PairingEvent, PairingInitiator, TerminalCause};
    use nessa_server::conversation::infrastructure::LocalPersonLinks;
    let fixture = Fixture::new().await;
    let (_metadata, _) = grant_store(&fixture);
    for (name, seed) in [("first", 53), ("second", 54)] {
        let (id, key) = claim(&fixture, name, true, seed).await;
        fixture
            .gateway
            .approve(&fixture.session, id, key, OsEntropy)
            .await
            .unwrap();
    }
    let mut facts = Vec::new();
    fixture
        .registry
        .with_current_links(&mut |current| {
            facts = current.to_vec();
            Ok(())
        })
        .unwrap();
    assert_eq!(facts.len(), 2);
    assert_ne!(facts[0].credential.id(), facts[1].credential.id());
    let invalid = &mut facts[1];
    let revoked_at = invalid.credential.issued_at() + 1;
    let owner = invalid.record.intent().owner().clone();
    let revocation = invalid
        .credential
        .revoke(
            revoked_at,
            nessa_auth::domain::Initiator::Principal(owner.clone()),
        )
        .unwrap()
        .unwrap();
    invalid.record = invalid
        .record
        .transition(
            PairingEvent::end(TerminalCause::Cancelled),
            PairingInitiator::Principal(owner),
            (revoked_at + 1) * 1000,
        )
        .unwrap()
        .after()
        .clone();
    invalid.revocation = Some(
        nessa_auth::application::revocation::CredentialRevocation::restore(
            nessa_auth::application::dto::CredentialTransitionDto::record(&revocation, 1, 1, None),
            &invalid.credential,
        )
        .unwrap(),
    );
    let source = Arc::new(FiniteSource(facts));
    let root = private_root(fixture.directory.path(), "contradictory-terminal");
    let path = root.join("grants.sqlite3");
    let links = LocalPersonLinks::open(&path, source.clone(), fixture.time.clone()).unwrap();
    assert_eq!(links.reconcile(&source.0), Err(PersonLinkError::Conflict));
    assert_eq!(link_count(&path), 0);
    assert_eq!(audit_count(&path), 0);
    fixture.gateway.shutdown().await;
}
