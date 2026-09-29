//! Passive read admission against the local ownership adapter.

use crate::conversation::application::{
    AdmitPassiveRead, ConversationRepository, ReadRefusal, ReceiverAuthority, ReceiverBinding,
    ReceiverReadScope,
};
use crate::{
    agents::domain::AgentId,
    conversation::{
        domain::{Conversation, ConversationApprovalMode, ConversationId, ConversationModelId},
        infrastructure::LocalConversationStore,
    },
};
use nessa_auth::{
    adapters::cedar::CedarPolicyEvaluator,
    application::{
        authorization::AuthorizeAction,
        ports::{
            AccessReader, AccessSnapshot, Clock, CredentialEvidence, CredentialVerifier,
            PortFuture, VerifiedCredential,
        },
        session::AuthenticateSession,
    },
    domain::{
        Action, AudienceId, Credential, CredentialId, Grant, Membership, MembershipId,
        MembershipRole, MembershipStatus, OrganizationId, PrincipalId, Resource, ResourceId,
    },
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

struct Access(Mutex<AccessSnapshot>);
impl AccessReader for Access {
    fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async { Ok(self.0.lock().unwrap().clone()) })
    }
}
impl CredentialVerifier for Access {
    fn verify<'a>(
        &'a self,
        _: &'a CredentialEvidence,
        _: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async {
            Ok(VerifiedCredential {
                credential_id: CredentialId::new("credential").unwrap(),
                expires_at: None,
            })
        })
    }
}
struct FixedClock;
impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        150_000
    }
}
struct Bindings(Mutex<Result<Option<ReceiverBinding>, ReadRefusal>>);
impl ReceiverAuthority for Bindings {
    fn resolve<'a>(
        &'a self,
        _: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        Box::pin(async { self.0.lock().unwrap().clone() })
    }
}
fn access() -> Access {
    let org = OrganizationId::new("org").unwrap();
    let owner = PrincipalId::new("owner").unwrap();
    let resource = Resource::new(org.clone(), ResourceId::new("gateway").unwrap());
    Access(Mutex::new(AccessSnapshot {
        credential: Credential::new(
            CredentialId::new("credential").unwrap(),
            owner.clone(),
            org.clone(),
            AudienceId::new("gateway").unwrap(),
            100,
            200,
            vec![Grant::new(
                Action::new("conversation.read").unwrap(),
                resource,
            )],
        )
        .unwrap(),
        membership: Membership::new(
            MembershipId::new("member").unwrap(),
            owner,
            org,
            MembershipRole::Member,
            MembershipStatus::Active,
        ),
        revision: 1,
    }))
}
fn binding() -> ReceiverBinding {
    ReceiverBinding {
        receiver_id: "receiver".into(),
        credential_id: CredentialId::new("credential").unwrap(),
        organization_id: OrganizationId::new("org").unwrap(),
        owner_id: PrincipalId::new("owner").unwrap(),
        access_epoch: 7,
        active: true,
    }
}

#[tokio::test]
async fn every_refusal_precedes_source_and_valid_owner_reaches_it() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let conversations = LocalConversationStore::open(&path).unwrap();
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    conversations
        .create(
            Conversation::new(
                id.clone(),
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("owner").unwrap(),
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
    let access = access();
    let clock = FixedClock;
    let policy = CedarPolicyEvaluator::new().unwrap();
    let gateway = Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    );
    let bindings = Bindings(Mutex::new(Ok(Some(binding()))));
    let session = AuthenticateSession {
        verifier: &access,
        access: &access,
        clock: &clock,
    }
    .execute(
        &CredentialEvidence::new(b"secret".to_vec()).unwrap(),
        &AudienceId::new("gateway").unwrap(),
    )
    .await
    .unwrap();
    let admit = AdmitPassiveRead {
        authorization: AuthorizeAction {
            access: &access,
            clock: &clock,
            policy: &policy,
        },
        gateway: &gateway,
        receivers: &bindings,
        conversations: &conversations,
    };
    let calls = AtomicUsize::new(0);
    let source = |_: ReceiverReadScope| {
        calls.fetch_add(1, Ordering::SeqCst);
        async { Ok::<(), ()>(()) }
    };
    assert_eq!(
        admit.read_with(&session, &id, "receiver", 7, source).await,
        Ok(())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        admit
            .read_with(&session, &id, "receiver", 7, |_| async {
                Err::<(), _>(ReadRefusal::Forbidden)
            })
            .await,
        Err(ReadRefusal::Unverifiable)
    );
    assert_eq!(
        admit
            .catalogue_with(&session, "receiver", 7, |_| async {
                Err::<(), _>(ReadRefusal::WrongOwner)
            })
            .await,
        Err(ReadRefusal::Unverifiable)
    );
    assert_eq!(
        admit
            .read_with(&session, &id, "receiver", 7, |_| {
                *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
                    active: false,
                    ..binding()
                }));
                async { Ok::<(), ()>(()) }
            })
            .await,
        Ok(())
    );
    assert_eq!(
        admit
            .read_with(&session, &id, "receiver", 7, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), ()>(()) }
            })
            .await,
        Err(ReadRefusal::Unauthorized)
    );
    *bindings.0.lock().unwrap() = Ok(Some(binding()));

    for (receiver, epoch, expected) in [
        ("wrong", 7, ReadRefusal::WrongReceiver),
        ("receiver", 6, ReadRefusal::StaleEpoch),
        ("", 7, ReadRefusal::InvalidRequest),
    ] {
        assert_eq!(
            admit
                .read_with(&session, &id, receiver, epoch, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), ()>(()) }
                })
                .await,
            Err(expected)
        );
    }
    let other = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    conversations
        .create(
            Conversation::new(
                other.clone(),
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("another-owner").unwrap(),
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
    assert_eq!(
        admit
            .read_with(&session, &other, "receiver", 7, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), ()>(()) }
            })
            .await,
        Err(ReadRefusal::WrongOwner)
    );
    let catalogue = admit.catalogue(&session, "receiver", 7).await.unwrap();
    assert_eq!(catalogue.owner_id, PrincipalId::new("owner").unwrap());
    assert_eq!(
        catalogue.organization_id,
        OrganizationId::new("org").unwrap()
    );
    assert_eq!(
        admit
            .catalogue_with(&session, "wrong", 7, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), ()>(()) }
            })
            .await,
        Err(ReadRefusal::WrongReceiver)
    );
    *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
        owner_id: PrincipalId::new("other-owner").unwrap(),
        ..binding()
    }));
    assert_eq!(
        admit
            .read_with(&session, &id, "receiver", 7, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), ()>(()) }
            })
            .await,
        Err(ReadRefusal::WrongOwner)
    );
    assert_eq!(
        admit
            .catalogue_with(&session, "receiver", 7, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), ()>(()) }
            })
            .await,
        Err(ReadRefusal::WrongOwner)
    );
    *bindings.0.lock().unwrap() = Err(ReadRefusal::Unverifiable);
    assert_eq!(
        admit
            .read_with(&session, &id, "receiver", 7, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), ()>(()) }
            })
            .await,
        Err(ReadRefusal::Unverifiable)
    );
    *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
        active: false,
        ..binding()
    }));
    assert_eq!(
        admit
            .read_with(&session, &id, "receiver", 7, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), ()>(()) }
            })
            .await,
        Err(ReadRefusal::Unauthorized)
    );
    *bindings.0.lock().unwrap() = Ok(Some(binding()));
    assert_eq!(
        admit
            .read_with(&session, &id, "receiver", 7, |_| {
                access
                    .0
                    .lock()
                    .unwrap()
                    .credential
                    .revoke(151, nessa_auth::domain::Initiator::LocalOperator)
                    .unwrap();
                async { Ok::<(), ()>(()) }
            })
            .await,
        Ok(())
    );
    assert_eq!(
        admit
            .read_with(&session, &id, "receiver", 7, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), ()>(()) }
            })
            .await,
        Err(ReadRefusal::Unauthorized)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
