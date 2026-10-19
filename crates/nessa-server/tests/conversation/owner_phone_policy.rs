//! Real signed Auth purpose and pinned TLS feed the Cedar-only scope decision.
//! This child module is hooked by its owner under the Cedar evaluator.
use super::*;
use crate::conversation::{domain::ReceiverBinding, infrastructure::LocalReceiverAuthority};
use crate::device_pairing::{
    application::{PairingReceivers, ReceiverDispatch},
    infrastructure::ConversationReceivers,
};
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        local::{BootstrapRequest, LocalCredentialStore},
        pairing::{verify_owner_phone_request, GatewayTrust, NativeIdentity, NativeTransport},
    },
    application::{
        dto::*,
        pairing::*,
        ports::{Clock, CredentialEvidence},
        session::{AuthenticateSession, AuthenticatedSession},
    },
    domain::{pairing::*, *},
};
use std::{
    net::{TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::Duration,
};
use zeroize::Zeroizing;
struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        110_001
    }
}
struct Fixture {
    _directory: tempfile::TempDir,
    purpose: OwnerPhonePurpose,
    session: AuthenticatedSession,
    bootstrap: AuthenticatedSession,
    receiver: ReceiverBinding,
    gateway: Resource,
    registry: Arc<LocalCredentialStore>,
    receivers: Arc<LocalReceiverAuthority>,
}
async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let parent = directory.path().canonicalize().unwrap();
    #[cfg(not(unix))]
    let parent = directory.path().to_path_buf();
    let registry_root = parent.join("registry");
    nessa_local_storage::create_directory(&registry_root).unwrap();
    let policy = Arc::new(CedarPolicyEvaluator::new().unwrap());
    let registry = Arc::new(
        LocalCredentialStore::open(&registry_root, "credentials.v1.json", policy.clone()).unwrap(),
    );
    let grants = [
        "credential.manage",
        "conversation.read",
        "conversation.write",
        "server.read",
    ]
    .into_iter()
    .map(|action| CredentialGrantDto {
        action: action.into(),
        resource: ResourceDto {
            organization_id: "org".into(),
            id: "gateway".into(),
        },
    })
    .collect();
    let issued = registry
        .bootstrap(BootstrapRequest {
            gateway_id: "gateway".into(),
            organization: OrganizationInputDto { id: "org".into() },
            principal: PrincipalInputDto {
                id: "owner".into(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: "member".into(),
                principal_id: "owner".into(),
                organization_id: "org".into(),
                role: MembershipRoleDto::Admin,
                state: MembershipStateDto::Active,
            },
            credential_id: "bootstrap".into(),
            issued_at: 100,
            expires_at: Some(100_000),
            grants,
        })
        .unwrap();
    let audience = AudienceId::new("gateway").unwrap();
    let bootstrap = AuthenticateSession {
        verifier: registry.as_ref(),
        access: registry.as_ref(),
        clock: &Time,
    }
    .execute(&issued.evidence, &audience)
    .await
    .unwrap();
    let gateway = Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    );
    let context = bootstrap.context();
    let source = OwnerPhoneSource::new(
        context.principal_id().clone(),
        context.membership_id().clone(),
        context.audience_id().clone(),
        context.credential_id().clone(),
        gateway.clone(),
    )
    .unwrap();
    let identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([42; 32]))).unwrap();
    let server_identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let pin = server_identity.public_spki();
    let original = OwnerPhoneRequest::new(
        OwnerPhoneRequestId::new([7; 16]).unwrap(),
        source,
        pin,
        "127.0.0.1:19744".parse().unwrap(),
        identity.device_key(),
    )
    .unwrap();
    let proof = verify_owner_phone_request(
        original.clone(),
        &identity.sign_owner_phone_request(&original).unwrap(),
    )
    .unwrap();
    let intent = ConsentIntent::new(
        ConsentIntentId::new([8; 16]),
        1,
        audience.clone(),
        context.principal_id().clone(),
        context.membership_id().clone(),
        gateway.clone(),
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    let initial = PairingRecord::new_owner_phone(
        InvitationId::new([9; 16]),
        intent.clone(),
        Time.unix_milliseconds(),
        PairingPolicy::initial(),
    )
    .unwrap();
    let admit = || async {
        AuthorizePairing {
            access: registry.as_ref(),
            policy: policy.as_ref(),
            clock: &Time,
        }
        .execute(&bootstrap, &intent, &gateway)
        .await
        .unwrap()
    };
    let approved = registry
        .authorize_owner_phone(&initial, &proof, &admit().await, &Time)
        .await
        .unwrap();
    let credential = CredentialId::new("phone-device").unwrap();
    registry
        .stage_pairing(
            approved.id(),
            credential.clone(),
            AttemptId::new([77; 16]),
            &admit().await,
            &Time,
        )
        .unwrap();
    let receiver_root = parent.join("receiver-access");
    nessa_local_storage::create_directory(&receiver_root).unwrap();
    let receivers = Arc::new(
        LocalReceiverAuthority::open(
            &receiver_root.join("receiver-access.sqlite3"),
            "policy",
            Arc::new(Time),
        )
        .unwrap(),
    );
    let real = ConversationReceivers::new(receivers.clone());
    let stage = registry.clone().acquire_stage(approved.id()).unwrap();
    let dispatch = ReceiverDispatch::for_pair(&stage).unwrap();
    let paired = real.pair(&dispatch).unwrap();
    registry
        .remember_receiver(approved.id(), &paired, &Time)
        .unwrap();
    registry
        .publish_pairing(approved.id(), &admit().await, &Time)
        .unwrap();
    drop(dispatch);
    drop(stage);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        NativeTransport::accept(socket, &server_identity).unwrap()
    });
    let socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let _client = NativeTransport::connect(socket, &identity, GatewayTrust::Pinned(pin)).unwrap();
    let channel = worker.join().unwrap();
    let verifier = registry.device_verifier(channel.device_proof());
    let session = AuthenticateSession {
        verifier: &verifier,
        access: registry.as_ref(),
        clock: &Time,
    }
    .execute(
        &CredentialEvidence::new(credential.as_str().as_bytes().to_vec()).unwrap(),
        &audience,
    )
    .await
    .unwrap();
    let purpose = registry
        .owner_phone_purpose(&session, &Time)
        .await
        .unwrap()
        .unwrap();
    let receiver = receivers.binding(&credential).unwrap().unwrap();
    Fixture {
        _directory: directory,
        purpose,
        session,
        bootstrap,
        receiver,
        gateway,
        registry,
        receivers,
    }
}
fn decide(
    e: &CedarOwnerPhonePolicyEvaluator,
    f: &Fixture,
    operation: OwnerPhoneOperation<'_>,
    receiver: &ReceiverBinding,
) -> Result<OwnerPhoneDecision, AccessError> {
    e.evaluate(OwnerPhonePolicyFacts {
        gateway: &f.gateway,
        context: f.session.context(),
        purpose: &f.purpose,
        receiver,
        operation,
    })
}
fn allow(result: Result<OwnerPhoneDecision, AccessError>) {
    match result.unwrap() {
        OwnerPhoneDecision::Allow { rules } => assert!(!rules.is_empty()),
        _ => panic!("expected policy permit"),
    }
}
fn deny(result: Result<OwnerPhoneDecision, AccessError>) {
    assert!(matches!(result.unwrap(), OwnerPhoneDecision::Deny { .. }))
}
#[tokio::test]
async fn owner_phone_cedar_admits_published_chat_methods_and_default_denies_administration() {
    let f = fixture().await;
    let e = CedarOwnerPhonePolicyEvaluator::new().unwrap();
    for method in [
        "conversation.create",
        "conversation.read",
        "conversation.send",
        "conversation.delete",
        "conversation.rename",
        "conversation.subscribe",
        "conversation.subscribeList",
        "conversation.subscribeEvents",
        "conversation.peek",
        "attachment.begin",
        "artifact.read",
        "agents.list",
        "server.health",
        "auth.session",
    ] {
        let action = nessa_protocol::product::generated::action_for_method(method);
        allow(decide(
            &e,
            &f,
            OwnerPhoneOperation::ProductMethod {
                method,
                published_action: action,
            },
            &f.receiver,
        ));
    }
    for method in [
        "agents.install",
        "mcp.servers.list",
        "credential.issue",
        "conversation.share",
        "pairing.approve",
        "peer.relay",
        "unknown.future",
    ] {
        deny(decide(
            &e,
            &f,
            OwnerPhoneOperation::ProductMethod {
                method,
                published_action: Some("conversation.write"),
            },
            &f.receiver,
        ));
    }
    deny(decide(
        &e,
        &f,
        OwnerPhoneOperation::ProductMethod {
            method: "conversation.send",
            published_action: Some("server.read"),
        },
        &f.receiver,
    ));
}
#[tokio::test]
async fn owner_phone_cedar_current_linkage_fails_closed_before_person_admission() {
    let f = fixture().await;
    let e = CedarOwnerPhonePolicyEvaluator::new().unwrap();
    let foreign = PrincipalId::new("foreign").unwrap();
    let otherorg = OrganizationId::new("otherorg").unwrap();
    allow(decide(&e, &f, OwnerPhoneOperation::Linkage, &f.receiver));
    for field in 0..5 {
        let mut receiver = f.receiver.clone();
        match field {
            0 => receiver.receiver_id = "foreign".into(),
            1 => receiver.owner_id = foreign.clone(),
            2 => receiver.organization_id = otherorg.clone(),
            3 => receiver.credential_id = CredentialId::new("foreign").unwrap(),
            _ => receiver.active = false,
        };
        deny(decide(
            &e,
            &f,
            OwnerPhoneOperation::ProductMethod {
                method: "conversation.send",
                published_action: Some("conversation.write"),
            },
            &receiver,
        ));
    }
    // Cedar transports full u64 epochs as raw strings; holding/history is separately joined.
    let mut maximum = f.receiver.clone();
    maximum.access_epoch = u64::MAX;
    allow(decide(
        &e,
        &f,
        OwnerPhoneOperation::ProductMethod {
            method: "conversation.send",
            published_action: Some("conversation.write"),
        },
        &maximum,
    ));
}
#[test]
fn owner_phone_cedar_bundle_validation_requires_unique_ids_and_strict_schema() {
    for (schema,policy) in [("{}",POLICIES),(SCHEMA,"permit(principal,action,resource);"),(SCHEMA,"@id(\"same\") permit(principal,action,resource); @id(\"same\") forbid(principal,action,resource);"),(SCHEMA,"@id(\"bad\") permit(principal,action,resource) when { principal.nonexistent == true };"),(SCHEMA,"not cedar")]{assert!(matches!(CedarOwnerPhonePolicyEvaluator::from_sources(schema,policy),Err(AccessError::Unavailable)));}
}
#[tokio::test]
async fn owner_phone_cedar_evaluation_diagnostics_never_become_allow() {
    let f = fixture().await;
    let policies="@id(\"arithmetic-overflow\") permit(principal is NessaPhone::Phone, action == NessaPhone::Action::\"product.request\", resource is NessaPhone::Scope) when { (9223372036854775807 + 1) > 0 };";
    let e = CedarOwnerPhonePolicyEvaluator::from_sources(SCHEMA, policies).unwrap();
    assert!(matches!(
        decide(
            &e,
            &f,
            OwnerPhoneOperation::ProductMethod {
                method: "conversation.send",
                published_action: Some("conversation.write")
            },
            &f.receiver
        ),
        Err(AccessError::Unavailable)
    ));
}

#[tokio::test]
async fn owner_phone_cedar_real_bootstrap_context_cannot_replace_delegated_context() {
    let f = fixture().await;
    let e = CedarOwnerPhonePolicyEvaluator::new().unwrap();
    deny(e.evaluate(OwnerPhonePolicyFacts {
        gateway: &f.gateway,
        context: f.bootstrap.context(),
        purpose: &f.purpose,
        receiver: &f.receiver,
        operation: OwnerPhoneOperation::ProductMethod {
            method: "conversation.send",
            published_action: Some("conversation.write"),
        },
    }));
    let other = Resource::new(
        f.gateway.organization_id().clone(),
        ResourceId::new("another-gateway").unwrap(),
    );
    deny(e.evaluate(OwnerPhonePolicyFacts {
        gateway: &other,
        context: f.session.context(),
        purpose: &f.purpose,
        receiver: &f.receiver,
        operation: OwnerPhoneOperation::ProductMethod {
            method: "conversation.send",
            published_action: Some("conversation.write"),
        },
    }));
}

#[tokio::test]
async fn owner_phone_creation_requires_ask_from_requested_or_canonical_default_facts() {
    let f = fixture().await;
    let evaluator = CedarOwnerPhonePolicyEvaluator::new().unwrap();
    for (requested_mode, default_mode) in
        [(None, "ask"), (Some("ask"), "ask"), (Some("ask"), "auto")]
    {
        allow(decide(
            &evaluator,
            &f,
            OwnerPhoneOperation::Creation {
                requested_mode,
                default_mode,
            },
            &f.receiver,
        ));
    }
    for (requested_mode, default_mode) in [
        (Some("auto"), "ask"),
        (Some("full"), "ask"),
        (None, "auto"),
        (None, "full"),
        (None, "future-default"),
        (Some("future-mode"), "ask"),
        (Some("Ask"), "ask"),
        (Some(""), "ask"),
    ] {
        deny(decide(
            &evaluator,
            &f,
            OwnerPhoneOperation::Creation {
                requested_mode,
                default_mode,
            },
            &f.receiver,
        ));
    }
    let mut inactive = f.receiver.clone();
    inactive.active = false;
    deny(decide(
        &evaluator,
        &f,
        OwnerPhoneOperation::Creation {
            requested_mode: None,
            default_mode: "ask",
        },
        &inactive,
    ));
}

#[tokio::test]
async fn owner_phone_submission_input_is_a_count_fact_with_no_host_path_permission() {
    let f = fixture().await;
    let e = CedarOwnerPhonePolicyEvaluator::new().unwrap();
    allow(decide(
        &e,
        &f,
        OwnerPhoneOperation::SubmissionInput { linked_files: 0 },
        &f.receiver,
    ));
    for linked_files in [1, 2, 1024] {
        deny(decide(
            &e,
            &f,
            OwnerPhoneOperation::SubmissionInput { linked_files },
            &f.receiver,
        ));
    }
    #[cfg(target_pointer_width = "64")]
    assert!(matches!(
        decide(
            &e,
            &f,
            OwnerPhoneOperation::SubmissionInput {
                linked_files: usize::MAX
            },
            &f.receiver
        ),
        Err(AccessError::Unavailable)
    ));
}
struct ActualHolding(Arc<LocalReceiverAuthority>);
impl crate::conversation::application::OwnerPhoneReceiverAuthority for ActualHolding {
    fn holding<'a>(
        &'a self,
        purpose: &'a OwnerPhonePurpose,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Option<ReceiverBinding>,
                        nessa_protocol::conversation::read_scope::ReadRefusal,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.0
                .holding(&crate::conversation::domain::PairedReceiver {
                    receiver_id: purpose.receiver().as_str().into(),
                    credential_id: purpose.credential().clone(),
                    organization_id: purpose.organization().clone(),
                    owner_id: purpose.owner().clone(),
                    paired_epoch: purpose.paired_epoch(),
                })
                .map_err(|_| nessa_protocol::conversation::read_scope::ReadRefusal::Unverifiable)
        })
    }
}
struct RefusedHolding(bool);
impl crate::conversation::application::OwnerPhoneReceiverAuthority for RefusedHolding {
    fn holding<'a>(
        &'a self,
        _: &'a OwnerPhonePurpose,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Option<ReceiverBinding>,
                        nessa_protocol::conversation::read_scope::ReadRefusal,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if self.0 {
                Err(nessa_protocol::conversation::read_scope::ReadRefusal::Unverifiable)
            } else {
                Ok(None)
            }
        })
    }
}
#[tokio::test]
async fn owner_phone_reader_factory_requires_actual_original_holding_without_owner_fallback() {
    use crate::conversation::application::AdmitOwnerPhoneReader;
    use nessa_protocol::conversation::read_scope::ReadRefusal;
    let f = fixture().await;
    let policy: Arc<dyn OwnerPhonePolicyEvaluator> =
        Arc::new(CedarOwnerPhonePolicyEvaluator::new().unwrap());
    let actual = ActualHolding(f.receivers.clone());
    let factory = AdmitOwnerPhoneReader {
        purposes: f.registry.as_ref(),
        holding: Some(&actual),
        clock: &Time,
        gateway: &f.gateway,
        policy: policy.clone(),
        access_policy: Some(crate::conversation_test_support::access_policy()),
    };
    let reader = factory.execute(&f.session).await.unwrap().unwrap();
    assert_eq!(reader.receiver(), &f.receiver);
    use crate::conversation::application::AccessAction;
    use crate::conversation::domain::{Conversation, ConversationDeletion};
    use nessa_protocol::{
        agents::AgentId,
        conversation::domain::{ConversationApprovalMode, ConversationId, ConversationModelId},
    };
    let foreign = PrincipalId::new("foreign").unwrap();
    let otherorg = OrganizationId::new("foreign-org").unwrap();
    let target = |owner: PrincipalId, organization: OrganizationId| {
        let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
        Conversation::new(
            id.clone(),
            organization,
            owner,
            "test".into(),
            id.to_string(),
            110_001,
            AgentId::Claude,
            ConversationModelId::new("model").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap()
    };
    let owned = target(f.purpose.owner().clone(), f.purpose.organization().clone());
    for action in [AccessAction::Read, AccessAction::Act, AccessAction::Locate] {
        assert_eq!(reader.admit_conversation(&owned, action), Ok(()));
        assert_eq!(
            reader.admit_conversation(
                &target(foreign.clone(), f.purpose.organization().clone()),
                action
            ),
            Err(ReadRefusal::WrongOwner)
        );
        assert_eq!(
            reader.admit_conversation(&target(f.purpose.owner().clone(), otherorg.clone()), action),
            Err(ReadRefusal::WrongOwner)
        );
    }
    assert_eq!(
        reader.admit_catalogue(
            f.purpose.owner(),
            f.purpose.organization(),
            AccessAction::List
        ),
        Ok(())
    );
    assert_eq!(
        reader.admit_catalogue(&foreign, f.purpose.organization(), AccessAction::List),
        Err(ReadRefusal::WrongOwner)
    );
    for deleted in [false, true] {
        assert_eq!(
            reader.admit_entry(f.purpose.owner(), f.purpose.organization(), deleted),
            Ok(())
        );
        assert_eq!(
            reader.admit_entry(&foreign, f.purpose.organization(), deleted),
            Err(ReadRefusal::WrongOwner)
        );
    }
    let deleted = owned
        .clone()
        .deleted(
            ConversationDeletion::new(
                f.purpose.organization().clone(),
                f.purpose.owner().clone(),
                "test".into(),
                "actual-delete".into(),
                110_002,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        reader.admit_conversation(&deleted, AccessAction::Locate),
        Ok(())
    );
    for action in [AccessAction::Read, AccessAction::Act] {
        assert_eq!(
            reader.admit_conversation(&deleted, action),
            Err(ReadRefusal::WrongOwner)
        );
    }
    assert_eq!(
        reader.admit_conversation(&owned, AccessAction::Erase),
        Err(ReadRefusal::WrongOwner)
    );
    assert_eq!(
        reader.admit_conversation(&deleted, AccessAction::Erase),
        Ok(())
    );
    let foreign_deleted = target(foreign.clone(), f.purpose.organization().clone())
        .deleted(
            ConversationDeletion::new(
                f.purpose.organization().clone(),
                foreign.clone(),
                "test".into(),
                "foreign-delete".into(),
                110_002,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        reader.admit_conversation(&foreign_deleted, AccessAction::Erase),
        Err(ReadRefusal::WrongOwner)
    );
    for (holding, expected) in [
        (None, ReadRefusal::Unverifiable),
        (
            Some(
                &RefusedHolding(false)
                    as &dyn crate::conversation::application::OwnerPhoneReceiverAuthority,
            ),
            ReadRefusal::Unauthorized,
        ),
        (
            Some(
                &RefusedHolding(true)
                    as &dyn crate::conversation::application::OwnerPhoneReceiverAuthority,
            ),
            ReadRefusal::Unverifiable,
        ),
    ] {
        let factory = AdmitOwnerPhoneReader {
            purposes: f.registry.as_ref(),
            holding,
            clock: &Time,
            gateway: &f.gateway,
            policy: policy.clone(),
            access_policy: Some(crate::conversation_test_support::access_policy()),
        };
        assert!(matches!(factory.execute(&f.session).await,Err(failure) if failure==expected));
        assert!(factory.execute(&f.bootstrap).await.unwrap().is_none());
    }
    // Actual original revocation history is authoritative even after a current binding returns.
    f.receivers
        .change(
            f.receiver.receiver_id.clone(),
            None,
            false,
            f.receiver.owner_id.clone(),
            "reader-revoke".into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        factory.execute(&f.session).await,
        Err(ReadRefusal::Unauthorized)
    ));
}

#[tokio::test]
async fn owner_phone_remember_is_refused_and_a_once_answer_stays_admitted() {
    let f = fixture().await;
    let evaluator = CedarOwnerPhonePolicyEvaluator::new().unwrap();
    match decide(&evaluator, &f, OwnerPhoneOperation::Remember, &f.receiver).unwrap() {
        OwnerPhoneDecision::Deny { rules } => {
            assert!(
                rules.iter().any(|rule| rule == "owner-phone.remember"),
                "{rules:?}"
            );
        }
        OwnerPhoneDecision::Allow { rules } => {
            panic!("a phone may not remember a permission rule: {rules:?}")
        }
    }
    allow(decide(
        &evaluator,
        &f,
        OwnerPhoneOperation::ProductMethod {
            method: "conversation.answer",
            published_action: Some("conversation.write"),
        },
        &f.receiver,
    ));
}

#[path = "owner_phone_catalogue.rs"]
mod catalogue;

#[path = "owner_phone_delete.rs"]
mod delete;

#[path = "owner_phone_remember.rs"]
mod remember;

#[path = "owner_phone_profile.rs"]
mod owner_phone_profile;

struct FixedPurpose<'a> {
    registry: &'a LocalCredentialStore,
    session: &'a AuthenticatedSession,
}
impl OwnerPhonePurposeSource for FixedPurpose<'_> {
    fn owner_phone_purpose<'a>(
        &'a self,
        _: &'a AuthenticatedSession,
        clock: &'a dyn Clock,
    ) -> nessa_auth::application::ports::PortFuture<'a, Option<OwnerPhonePurpose>> {
        // Each immutable purpose comes from the actual signed native session's
        // current registry query. The DI intentionally supplies it to another
        // session so the canonical linkage gate must refuse that contradiction.
        self.registry.owner_phone_purpose(self.session, clock)
    }
}
struct CountAccess {
    real: Arc<dyn crate::conversation::application::ConversationAccessPolicyEvaluator>,
    calls: std::sync::atomic::AtomicUsize,
}
impl crate::conversation::application::ConversationAccessPolicyEvaluator for CountAccess {
    fn evaluate(
        &self,
        ask: &crate::conversation::application::AccessAsk<'_>,
    ) -> crate::conversation::application::AccessDecision {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.real.evaluate(ask)
    }
}
#[tokio::test]
async fn owner_phone_person_subject_requires_current_linkage_before_shared_access() {
    use crate::conversation::application::{AccessAction, AdmitOwnerPhoneReader};
    use nessa_protocol::conversation::read_scope::ReadRefusal;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let f = fixture().await;
    let purposes = FixedPurpose {
        registry: f.registry.as_ref(),
        session: &f.session,
    };
    let actual = ActualHolding(f.receivers.clone());
    let shared = Arc::new(CountAccess {
        real: crate::conversation_test_support::access_policy(),
        calls: AtomicUsize::new(0),
    });
    let factory = AdmitOwnerPhoneReader {
        purposes: &purposes,
        holding: Some(&actual),
        clock: &Time,
        gateway: &f.gateway,
        policy: Arc::new(CedarOwnerPhonePolicyEvaluator::new().unwrap()),
        access_policy: Some(shared.clone()),
    };
    // Authenticated owner itself is a different credential from this native delegate.
    assert!(matches!(
        factory.execute(&f.bootstrap).await,
        Err(ReadRefusal::WrongOwner)
    ));
    assert_eq!(shared.calls.load(Ordering::SeqCst), 0);
    let reader = factory.execute(&f.session).await.unwrap().unwrap();
    assert_eq!(shared.calls.load(Ordering::SeqCst), 0);
    reader
        .admit_catalogue(
            f.purpose.owner(),
            f.purpose.organization(),
            AccessAction::List,
        )
        .unwrap();
    assert_eq!(shared.calls.load(Ordering::SeqCst), 1);
}

#[path = "owner_phone_refusal_trace.rs"]
mod refusal_trace;

#[tokio::test]
async fn owner_phone_access_owner_availability_follows_canonical_classification() {
    use crate::conversation::application::AdmitOwnerPhoneReader;
    use nessa_protocol::conversation::read_scope::ReadRefusal;
    let f = fixture().await;
    let factory = AdmitOwnerPhoneReader {
        purposes: f.registry.as_ref(),
        holding: None,
        clock: &Time,
        gateway: &f.gateway,
        policy: Arc::new(CedarOwnerPhonePolicyEvaluator::new().unwrap()),
        access_policy: None,
    };
    assert!(factory.execute(&f.bootstrap).await.unwrap().is_none());
    assert!(matches!(
        factory.execute(&f.session).await,
        Err(ReadRefusal::Unverifiable)
    ));
}
