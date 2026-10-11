//! Passive read admission against the local ownership adapter.

use crate::conversation::application::{
    AdmitPassiveRead, CatalogueReadError, CatalogueReadFuture, CatalogueReadOperation,
    CatalogueReadResponse, CatalogueReadSource, CatalogueReadValue, ConversationRepository,
    PassiveRead, PassiveReadGrants, ReadCatalogue, ReadRecords, ReceiverAuthority, ReceiverBinding,
    RecordHead, RecordReadError, RecordReadFuture, RecordReadLease, RecordReadOperation,
    RecordReadResponse, RecordReadSource, RecordReadValue,
};
use crate::conversation::domain::Conversation;
use crate::conversation::infrastructure::LocalConversationStore;
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::application::authorization::AuthorizeAction;
use nessa_auth::application::ports::{
    AccessError, AccessReader, AccessSnapshot, Clock, CredentialEvidence, CredentialVerifier,
    Decision, PolicyEvaluator, PortFuture, VerifiedCredential,
};
use nessa_auth::application::session::AuthenticateSession;
use nessa_auth::domain::{
    Action, AudienceId, AuthContext, Credential, CredentialId, Grant, Membership, MembershipId,
    MembershipRole, MembershipStatus, OrganizationId, PrincipalId, Resource, ResourceId,
};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    conversation_catalogue_stream, ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_protocol::conversation::read_scope::{
    CatalogueReadScope, ReadRefusal, ReceiverReadScope,
};
use nessa_sync::replication::catalogue::{
    CataloguePass, EntryKey, ManifestEntry, ManifestPage, ManifestRequest, ResolvedEntry,
};
use nessa_sync::replication::domain::{Id, Page, PageRequest, Scope};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
struct RecordSpy {
    reads: AtomicUsize,
    returned_scope: Mutex<Option<Scope>>,
    returned_page: Mutex<Option<PageRequest>>,
}

impl RecordReadSource for RecordSpy {
    fn read<'a>(
        &'a self,
        admitted: ReceiverReadScope,
        _: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let scope = self
                .returned_scope
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| {
                    Scope::new(
                        Id::new(&admitted.receiver_id).unwrap(),
                        Id::new("origin").unwrap(),
                        Id::new(admitted.conversation_id.to_string()).unwrap(),
                        Id::new("incarnation").unwrap(),
                        Id::new("schema").unwrap(),
                        Id::new("epoch-7").unwrap(),
                    )
                });
            let value = match self.returned_page.lock().unwrap().clone() {
                Some(request) => RecordReadValue::Page(Page {
                    request,
                    records: Vec::new(),
                }),
                None => RecordReadValue::Head(RecordHead { scope, head: 0 }),
            };
            Ok(RecordReadResponse { value, lease })
        })
    }
}

#[tokio::test]
async fn record_use_case_admits_before_metadata_and_rechecks_each_request() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let conversations = LocalConversationStore::open(&private.join("metadata.sqlite3")).unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
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
    let source = RecordSpy::default();
    let read = ReadRecords {
        admission: AdmitPassiveRead {
            authorization: AuthorizeAction {
                access: &access,
                clock: &clock,
                policy: &policy,
            },
            gateway: &gateway,
            receivers: &bindings,
            conversations: &conversations,
            grants: &CONVERSATION_READ_GRANT,
            read_grants: &crate::conversation_test_support::EveryConversationGranted,
        },
        source: &source,
    };
    for (receiver, epoch, refusal) in [
        ("wrong", 7, ReadRefusal::WrongReceiver),
        ("receiver", 6, ReadRefusal::StaleEpoch),
    ] {
        let result = read
            .execute(
                &session,
                &id,
                receiver,
                epoch,
                RecordReadOperation::Head,
                RecordReadLease::new(()),
            )
            .await;
        assert!(matches!(result, Err(RecordReadError::Admission(actual)) if actual == refusal));
    }
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    let result = read
        .execute(
            &session,
            &id,
            "receiver",
            7,
            RecordReadOperation::Head,
            RecordReadLease::new(()),
        )
        .await
        .unwrap();
    assert!(matches!(result.value, RecordReadValue::Head(head) if head.head == 0));
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    let id_value = |value: &str| Id::new(value).unwrap();
    let actual = Scope::new(
        id_value("receiver"),
        id_value("origin"),
        id_value(&id.to_string()),
        id_value("incarnation"),
        id_value("schema"),
        id_value("epoch-7"),
    );
    for (receiver, stream, epoch, expected) in [
        (
            "another",
            id.to_string(),
            "epoch-7",
            ReadRefusal::WrongReceiver,
        ),
        (
            "receiver",
            "another-conversation".into(),
            "epoch-7",
            ReadRefusal::WrongOwner,
        ),
        (
            "receiver",
            id.to_string(),
            "epoch-6",
            ReadRefusal::StaleEpoch,
        ),
    ] {
        *source.returned_scope.lock().unwrap() = Some(Scope::new(
            id_value(receiver),
            id_value("origin"),
            id_value(&stream),
            id_value("incarnation"),
            id_value("schema"),
            id_value(epoch),
        ));
        let result = read
            .execute(
                &session,
                &id,
                "receiver",
                7,
                RecordReadOperation::Head,
                RecordReadLease::new(()),
            )
            .await;
        assert!(matches!(result, Err(RecordReadError::Admission(reason)) if reason == expected));
    }
    *source.returned_scope.lock().unwrap() = None;
    let page = PageRequest {
        scope: actual,
        after: 0,
        target: 1,
        max_records: 1,
        max_payload_bytes: 1,
        max_record_bytes: 1,
    };
    // A Head result cannot satisfy a Page operation, and conversely.
    let result = read
        .execute(
            &session,
            &id,
            "receiver",
            7,
            RecordReadOperation::Page(page.clone()),
            RecordReadLease::new(()),
        )
        .await;
    assert!(matches!(
        result,
        Err(RecordReadError::Admission(ReadRefusal::Unverifiable))
    ));
    *source.returned_page.lock().unwrap() = Some(page);
    let result = read
        .execute(
            &session,
            &id,
            "receiver",
            7,
            RecordReadOperation::Head,
            RecordReadLease::new(()),
        )
        .await;
    assert!(matches!(
        result,
        Err(RecordReadError::Admission(ReadRefusal::Unverifiable))
    ));
    *source.returned_page.lock().unwrap() = None;
    let calls_before_revoke = source.reads.load(Ordering::SeqCst);
    *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
        active: false,
        ..binding()
    }));
    let result = read
        .execute(
            &session,
            &id,
            "receiver",
            7,
            RecordReadOperation::Head,
            RecordReadLease::new(()),
        )
        .await;
    assert!(matches!(
        result,
        Err(RecordReadError::Admission(ReadRefusal::Unauthorized))
    ));
    assert_eq!(source.reads.load(Ordering::SeqCst), calls_before_revoke);
}

struct Access(Mutex<AccessSnapshot>);
impl AccessReader for Access {
    fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async { Ok(self.0.lock().unwrap().clone()) })
    }
}

struct FailingAccess(AccessError);
impl AccessReader for FailingAccess {
    fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async { Err(self.0) })
    }
}

#[tokio::test]
async fn passive_admission_preserves_unverifiable_authority_failures() {
    let access = access();
    let clock = FixedClock;
    let policy = CedarPolicyEvaluator::new().unwrap();
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
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let conversations = LocalConversationStore::open(&private.join("metadata.sqlite3")).unwrap();
    let gateway = Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    );
    let bindings = Bindings(Mutex::new(Ok(Some(binding()))));
    for (error, expected) in [
        (AccessError::Denied, ReadRefusal::Forbidden),
        (AccessError::StaleRevision, ReadRefusal::Unverifiable),
        (AccessError::Unavailable, ReadRefusal::Unverifiable),
        (AccessError::Unsupported, ReadRefusal::Unverifiable),
        (AccessError::InvalidCredential, ReadRefusal::Unauthorized),
        (AccessError::CredentialRevoked, ReadRefusal::Unauthorized),
        (AccessError::CredentialExpired, ReadRefusal::Unauthorized),
        (AccessError::InactiveMembership, ReadRefusal::Unauthorized),
        (AccessError::IdentityMismatch, ReadRefusal::Unauthorized),
    ] {
        let failing = FailingAccess(error);
        let admit = AdmitPassiveRead {
            authorization: AuthorizeAction {
                access: &failing,
                clock: &clock,
                policy: &policy,
            },
            gateway: &gateway,
            receivers: &bindings,
            conversations: &conversations,
            grants: &CONVERSATION_READ_GRANT,
            read_grants: &crate::conversation_test_support::EveryConversationGranted,
        };
        let calls = AtomicUsize::new(0);
        assert_eq!(
            admit
                .catalogue_with(&session, "receiver", 7, PassiveRead::CatalogueHead, |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), ()>(()) }
                })
                .await,
            Err(expected),
            "{error:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
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
struct ConversationReadGrant;

impl PassiveReadGrants for ConversationReadGrant {
    fn grant(&self, _: PassiveRead) -> Option<&'static str> {
        Some("conversation.read")
    }
}

static CONVERSATION_READ_GRANT: ConversationReadGrant = ConversationReadGrant;

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
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
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
        grants: &CONVERSATION_READ_GRANT,
        read_grants: &crate::conversation_test_support::EveryConversationGranted,
    };
    let calls = AtomicUsize::new(0);
    let source = |_: ReceiverReadScope| {
        calls.fetch_add(1, Ordering::SeqCst);
        async { Ok::<(), ()>(()) }
    };
    assert_eq!(
        admit
            .read_with(
                &session,
                &id,
                "receiver",
                7,
                PassiveRead::RecordHead,
                source
            )
            .await,
        Ok(())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        admit
            .read_with(
                &session,
                &id,
                "receiver",
                7,
                PassiveRead::RecordHead,
                |_| async { Err::<(), _>(ReadRefusal::Forbidden) }
            )
            .await,
        Err(ReadRefusal::Unverifiable)
    );
    assert_eq!(
        admit
            .catalogue_with(
                &session,
                "receiver",
                7,
                PassiveRead::CatalogueHead,
                |_| async { Err::<(), _>(ReadRefusal::WrongOwner) }
            )
            .await,
        Err(ReadRefusal::Unverifiable)
    );
    assert_eq!(
        admit
            .read_with(
                &session,
                &id,
                "receiver",
                7,
                PassiveRead::RecordHead,
                |_| {
                    *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
                        active: false,
                        ..binding()
                    }));
                    async { Ok::<(), ()>(()) }
                }
            )
            .await,
        Ok(())
    );
    assert_eq!(
        admit
            .read_with(
                &session,
                &id,
                "receiver",
                7,
                PassiveRead::RecordHead,
                |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), ()>(()) }
                }
            )
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
                .read_with(
                    &session,
                    &id,
                    receiver,
                    epoch,
                    PassiveRead::RecordHead,
                    |_| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        async { Ok::<(), ()>(()) }
                    }
                )
                .await,
            Err(expected)
        );
    }
    let other = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
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
            .read_with(
                &session,
                &other,
                "receiver",
                7,
                PassiveRead::RecordHead,
                |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), ()>(()) }
                }
            )
            .await,
        Err(ReadRefusal::WrongOwner)
    );
    let catalogue = admit
        .catalogue(&session, "receiver", 7, PassiveRead::CatalogueHead)
        .await
        .unwrap();
    assert_eq!(catalogue.owner_id, PrincipalId::new("owner").unwrap());
    assert_eq!(
        catalogue.organization_id,
        OrganizationId::new("org").unwrap()
    );
    assert_eq!(
        admit
            .catalogue_with(&session, "wrong", 7, PassiveRead::CatalogueHead, |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), ()>(()) }
            })
            .await,
        Err(ReadRefusal::WrongReceiver)
    );
    // A binding names its grantor, who need not be the session's principal
    // (a peer gateway signs in as itself). Its reads are the grantor's: a
    // conversation that is not the grantor's is refused, and its catalogue
    // is the grantor's, narrowed to the receiver's grants by the store.
    *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
        owner_id: PrincipalId::new("other-owner").unwrap(),
        ..binding()
    }));
    assert_eq!(
        admit
            .read_with(
                &session,
                &id,
                "receiver",
                7,
                PassiveRead::RecordHead,
                |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), ()>(()) }
                }
            )
            .await,
        Err(ReadRefusal::WrongOwner)
    );
    assert_eq!(
        admit
            .catalogue(&session, "receiver", 7, PassiveRead::CatalogueHead)
            .await
            .unwrap()
            .owner_id,
        PrincipalId::new("other-owner").unwrap()
    );
    *bindings.0.lock().unwrap() = Err(ReadRefusal::Unverifiable);
    assert_eq!(
        admit
            .read_with(
                &session,
                &id,
                "receiver",
                7,
                PassiveRead::RecordHead,
                |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), ()>(()) }
                }
            )
            .await,
        Err(ReadRefusal::Unverifiable)
    );
    *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
        active: false,
        ..binding()
    }));
    assert_eq!(
        admit
            .read_with(
                &session,
                &id,
                "receiver",
                7,
                PassiveRead::RecordHead,
                |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), ()>(()) }
                }
            )
            .await,
        Err(ReadRefusal::Unauthorized)
    );
    *bindings.0.lock().unwrap() = Ok(Some(binding()));
    assert_eq!(
        admit
            .read_with(
                &session,
                &id,
                "receiver",
                7,
                PassiveRead::RecordHead,
                |_| {
                    access
                        .0
                        .lock()
                        .unwrap()
                        .credential
                        .revoke(151, nessa_auth::domain::Initiator::LocalOperator)
                        .unwrap();
                    async { Ok::<(), ()>(()) }
                }
            )
            .await,
        Ok(())
    );
    assert_eq!(
        admit
            .read_with(
                &session,
                &id,
                "receiver",
                7,
                PassiveRead::RecordHead,
                |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), ()>(()) }
                }
            )
            .await,
        Err(ReadRefusal::Unauthorized)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

struct CatalogueSpy {
    reads: AtomicUsize,
    reply: Mutex<Option<CatalogueReadValue>>,
}
impl CatalogueReadSource for CatalogueSpy {
    fn read(
        &self,
        _: CatalogueReadScope,
        _: CatalogueReadOperation,
        lease: RecordReadLease,
    ) -> CatalogueReadFuture<'_> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(CatalogueReadResponse {
                value: self.reply.lock().unwrap().take().expect("fixture reply"),
                lease,
            })
        })
    }
}

#[tokio::test]
async fn catalogue_use_case_correlates_admitted_selector_and_operation_before_returning_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let conversations = LocalConversationStore::open(&private.join("metadata.sqlite3")).unwrap();
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
    let source = CatalogueSpy {
        reads: AtomicUsize::new(0),
        reply: Mutex::new(None),
    };
    let read = ReadCatalogue {
        admission: AdmitPassiveRead {
            authorization: AuthorizeAction {
                access: &access,
                clock: &clock,
                policy: &policy,
            },
            gateway: &gateway,
            receivers: &bindings,
            conversations: &conversations,
            grants: &CONVERSATION_READ_GRANT,
            read_grants: &crate::conversation_test_support::EveryConversationGranted,
        },
        source: &source,
    };
    let id = |value: &str| Id::new(value).unwrap();
    let parts = [
        id("receiver"),
        id("origin"),
        conversation_catalogue_stream(&binding().organization_id, &binding().owner_id),
        id("incarnation"),
        id("schema"),
        id("epoch-7"),
    ];
    let scope = |parts: &[Id; 6]| {
        Scope::new(
            parts[0].clone(),
            parts[1].clone(),
            parts[2].clone(),
            parts[3].clone(),
            parts[4].clone(),
            parts[5].clone(),
        )
    };
    for (index, refusal) in [
        (0, ReadRefusal::WrongReceiver),
        (2, ReadRefusal::WrongOwner),
        (5, ReadRefusal::StaleEpoch),
    ] {
        let mut foreign = parts.clone();
        foreign[index] = id("foreign");
        *source.reply.lock().unwrap() = Some(CatalogueReadValue::Head {
            scope: scope(&foreign),
            head: 5,
        });
        assert!(
            matches!(read.execute(&session,"receiver",7,CatalogueReadOperation::Head,RecordReadLease::new(())).await,
            Err(CatalogueReadError::Admission(actual)) if actual == refusal)
        );
    }
    // Origin, incarnation and schema are physical facts; the injected adapter
    // supplies these, while application correlation owns the admitted selector.
    *source.reply.lock().unwrap() = Some(CatalogueReadValue::Head {
        scope: scope(&parts),
        head: 5,
    });
    assert!(matches!(
        read.execute(
            &session,
            "receiver",
            7,
            CatalogueReadOperation::Head,
            RecordReadLease::new(())
        )
        .await
        .unwrap()
        .value,
        CatalogueReadValue::Head { head: 5, .. }
    ));
    let request = ManifestRequest {
        pass: CataloguePass {
            scope: scope(&parts),
            completed: 0,
            boundary: 5,
            cursor: None,
            generation: 1,
        },
        max_entries: 1,
    };
    *source.reply.lock().unwrap() = Some(CatalogueReadValue::Head {
        scope: scope(&parts),
        head: 5,
    });
    assert!(matches!(
        read.execute(
            &session,
            "receiver",
            7,
            CatalogueReadOperation::Manifest(request.clone()),
            RecordReadLease::new(())
        )
        .await,
        Err(CatalogueReadError::Admission(ReadRefusal::Unverifiable))
    ));
    let mut changed = request.clone();
    changed.pass.generation = 2;
    *source.reply.lock().unwrap() = Some(CatalogueReadValue::Manifest(ManifestPage {
        request: changed,
        entries: vec![],
        has_more: false,
    }));
    assert!(matches!(
        read.execute(
            &session,
            "receiver",
            7,
            CatalogueReadOperation::Manifest(request.clone()),
            RecordReadLease::new(())
        )
        .await,
        Err(CatalogueReadError::Admission(ReadRefusal::Unverifiable))
    ));
    *source.reply.lock().unwrap() = Some(CatalogueReadValue::Manifest(ManifestPage {
        request: request.clone(),
        entries: vec![],
        has_more: false,
    }));
    assert!(read
        .execute(
            &session,
            "receiver",
            7,
            CatalogueReadOperation::Manifest(request.clone()),
            RecordReadLease::new(())
        )
        .await
        .is_ok());
    let descriptor = ManifestEntry {
        key: EntryKey {
            creation: 1,
            id: id("entry"),
        },
        revision: 2,
        deleted: false,
    };
    *source.reply.lock().unwrap() = Some(CatalogueReadValue::Resolve(ResolvedEntry {
        manifest: descriptor.clone(),
        payload: vec![1],
    }));
    assert!(read
        .execute(
            &session,
            "receiver",
            7,
            CatalogueReadOperation::Resolve {
                pass: request.pass.clone(),
                descriptor,
                max_payload_bytes: 1
            },
            RecordReadLease::new(())
        )
        .await
        .is_ok());
    let calls = source.reads.load(Ordering::SeqCst);
    let mut foreign = request;
    foreign.pass.scope = {
        let mut foreign = parts;
        foreign[0] = id("foreign");
        scope(&foreign)
    };
    assert!(matches!(
        read.execute(
            &session,
            "receiver",
            7,
            CatalogueReadOperation::Manifest(foreign),
            RecordReadLease::new(())
        )
        .await,
        Err(CatalogueReadError::Admission(ReadRefusal::WrongReceiver))
    ));
    *bindings.0.lock().unwrap() = Ok(Some(ReceiverBinding {
        active: false,
        ..binding()
    }));
    assert!(matches!(
        read.execute(
            &session,
            "receiver",
            7,
            CatalogueReadOperation::Head,
            RecordReadLease::new(())
        )
        .await,
        Err(CatalogueReadError::Admission(ReadRefusal::Unauthorized))
    ));
    assert_eq!(source.reads.load(Ordering::SeqCst), calls);
}

/// A page is not admitted on the head grant, and a manifest or resolve is not
/// admitted on the catalogue-head grant. The operation being executed selects
/// the grant.
#[tokio::test]
async fn each_operation_asks_for_its_own_grant() {
    let asked = AskedActions(Mutex::new(Vec::new()));
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let conversations = LocalConversationStore::open(&private.join("metadata.sqlite3")).unwrap();
    let access = access();
    let clock = FixedClock;
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
    let admission = AdmitPassiveRead {
        authorization: AuthorizeAction {
            access: &access,
            clock: &clock,
            policy: &asked,
        },
        gateway: &gateway,
        receivers: &bindings,
        conversations: &conversations,
        grants: &OPERATION_GRANTS,
        read_grants: &crate::conversation_test_support::EveryConversationGranted,
    };
    let records = RecordSpy::default();
    let read = ReadRecords {
        admission,
        source: &records,
    };
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let scope = Scope::new(
        Id::new("receiver").unwrap(),
        Id::new("origin").unwrap(),
        Id::new("stream").unwrap(),
        Id::new("incarnation").unwrap(),
        Id::new("schema").unwrap(),
        Id::new("epoch-7").unwrap(),
    );
    let page = PageRequest {
        scope: scope.clone(),
        after: 0,
        target: 1,
        max_records: 1,
        max_payload_bytes: 1,
        max_record_bytes: 1,
    };
    for (operation, grant) in [
        (RecordReadOperation::Head, "record.head"),
        (RecordReadOperation::Page(page), "record.page"),
    ] {
        asked.0.lock().unwrap().clear();
        let result = read
            .execute(
                &session,
                &id,
                "receiver",
                7,
                operation,
                RecordReadLease::new(()),
            )
            .await;
        assert!(matches!(
            result,
            Err(RecordReadError::Admission(ReadRefusal::Forbidden))
        ));
        assert_eq!(asked.0.lock().unwrap().as_slice(), &[grant]);
    }
    assert_eq!(records.reads.load(Ordering::SeqCst), 0);

    let catalogues = CatalogueSpy {
        reads: AtomicUsize::new(0),
        reply: Mutex::new(None),
    };
    let admission = AdmitPassiveRead {
        authorization: AuthorizeAction {
            access: &access,
            clock: &clock,
            policy: &asked,
        },
        gateway: &gateway,
        receivers: &bindings,
        conversations: &conversations,
        grants: &OPERATION_GRANTS,
        read_grants: &crate::conversation_test_support::EveryConversationGranted,
    };
    let catalogue = ReadCatalogue {
        admission,
        source: &catalogues,
    };
    let pass = CataloguePass {
        scope,
        completed: 0,
        boundary: 5,
        cursor: None,
        generation: 1,
    };
    let descriptor = ManifestEntry {
        key: EntryKey {
            creation: 1,
            id: Id::new("entry").unwrap(),
        },
        revision: 2,
        deleted: false,
    };
    let operations = [
        (CatalogueReadOperation::Head, "catalogue.head"),
        (
            CatalogueReadOperation::Manifest(ManifestRequest {
                pass: pass.clone(),
                max_entries: 1,
            }),
            "catalogue.manifest",
        ),
        (
            CatalogueReadOperation::Resolve {
                pass,
                descriptor,
                max_payload_bytes: 1,
            },
            "catalogue.resolve",
        ),
    ];
    for (operation, grant) in operations {
        asked.0.lock().unwrap().clear();
        let result = catalogue
            .execute(&session, "receiver", 7, operation, RecordReadLease::new(()))
            .await;
        assert!(
            matches!(
                result,
                Err(CatalogueReadError::Admission(ReadRefusal::Forbidden))
            ),
            "{grant}"
        );
        assert_eq!(asked.0.lock().unwrap().as_slice(), &[grant], "{grant}");
    }
    assert_eq!(catalogues.reads.load(Ordering::SeqCst), 0);
}

struct OperationGrants;

impl PassiveReadGrants for OperationGrants {
    fn grant(&self, read: PassiveRead) -> Option<&'static str> {
        Some(match read {
            PassiveRead::RecordHead => "record.head",
            PassiveRead::RecordPage => "record.page",
            PassiveRead::CatalogueHead => "catalogue.head",
            PassiveRead::CatalogueManifest => "catalogue.manifest",
            PassiveRead::CatalogueResolve => "catalogue.resolve",
        })
    }
}

static OPERATION_GRANTS: OperationGrants = OperationGrants;

struct AskedActions(Mutex<Vec<&'static str>>);

impl PolicyEvaluator for AskedActions {
    fn evaluate(
        &self,
        _: &AuthContext,
        action: &Action,
        _: &Resource,
        _: &AccessSnapshot,
    ) -> Result<Decision, AccessError> {
        let grant = match action.as_str() {
            "record.head" => "record.head",
            "record.page" => "record.page",
            "catalogue.head" => "catalogue.head",
            "catalogue.manifest" => "catalogue.manifest",
            "catalogue.resolve" => "catalogue.resolve",
            other => panic!("unexpected grant {other}"),
        };
        self.0.lock().unwrap().push(grant);
        Ok(Decision::Deny)
    }
}

// Read grants (issue 704): rows G1, G3 and G4 of docs/design/read-grants.md.
// The store is both the ownership repository and the grant table, as
// composition wires it.

struct GrantFixture {
    _directory: tempfile::TempDir,
    store: LocalConversationStore,
    id: ConversationId,
}
impl GrantFixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let store = LocalConversationStore::open(&private.join("metadata.sqlite3")).unwrap();
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        store
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
        Self {
            _directory: directory,
            store,
            id,
        }
    }
    async fn change(&self, transition: crate::conversation::application::ReadGrantTransition) {
        use crate::conversation::application::ReadGrants;
        assert!(self
            .store
            .change(crate::conversation::application::ReadGrantChange {
                transition,
                conversation_id: self.id.clone(),
                receiver_id: Some("receiver".into()),
                credential_id: CredentialId::new("credential").unwrap(),
                initiator: crate::conversation::application::ConversationCaller {
                    organization_id: OrganizationId::new("org").unwrap(),
                    principal_id: PrincipalId::new("owner").unwrap(),
                    surface_id: "desktop".into(),
                    action_id: uuid::Uuid::new_v4().to_string(),
                },
                at_ms: 1,
            })
            .await
            .unwrap());
    }
}

async fn device_session() -> nessa_auth::application::session::AuthenticatedSession {
    let access = access();
    AuthenticateSession {
        verifier: &access,
        access: &access,
        clock: &FixedClock,
    }
    .execute(
        &CredentialEvidence::new(b"secret".to_vec()).unwrap(),
        &AudienceId::new("gateway").unwrap(),
    )
    .await
    .unwrap()
}

/// Rows G1 (refused before its source, for every record read and the head a
/// records watch is admitted as) and G3 (granted, then admitted).
#[tokio::test]
async fn an_ungranted_conversation_is_refused_before_its_source() {
    let fixture = GrantFixture::new().await;
    let access = access();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let gateway = Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    );
    let bindings = Bindings(Mutex::new(Ok(Some(binding()))));
    let admit = AdmitPassiveRead {
        authorization: AuthorizeAction {
            access: &access,
            clock: &FixedClock,
            policy: &policy,
        },
        gateway: &gateway,
        receivers: &bindings,
        conversations: &fixture.store,
        grants: &CONVERSATION_READ_GRANT,
        read_grants: &fixture.store,
    };
    let session = device_session().await;
    let calls = AtomicUsize::new(0);
    for read in [PassiveRead::RecordHead, PassiveRead::RecordPage] {
        let source = |_: ReceiverReadScope| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok::<(), ()>(()) }
        };
        assert_eq!(
            admit
                .read_with(&session, &fixture.id, "receiver", 7, read, source)
                .await,
            Err(ReadRefusal::WrongOwner)
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no source was touched");
    fixture
        .change(crate::conversation::application::ReadGrantTransition::Grant)
        .await;
    for read in [PassiveRead::RecordHead, PassiveRead::RecordPage] {
        assert!(admit
            .execute(&session, &fixture.id, "receiver", 7, read)
            .await
            .is_ok());
    }
}

/// Row G4: revocation ends the next read; a read already admitted finishes.
#[tokio::test]
async fn a_revoke_ends_the_next_read_and_lets_an_admitted_one_finish() {
    let fixture = GrantFixture::new().await;
    fixture
        .change(crate::conversation::application::ReadGrantTransition::Grant)
        .await;
    let access = access();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let gateway = Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    );
    let bindings = Bindings(Mutex::new(Ok(Some(binding()))));
    let admit = AdmitPassiveRead {
        authorization: AuthorizeAction {
            access: &access,
            clock: &FixedClock,
            policy: &policy,
        },
        gateway: &gateway,
        receivers: &bindings,
        conversations: &fixture.store,
        grants: &CONVERSATION_READ_GRANT,
        read_grants: &fixture.store,
    };
    let session = device_session().await;
    let finished = admit
        .read_with(
            &session,
            &fixture.id,
            "receiver",
            7,
            PassiveRead::RecordPage,
            |_: ReceiverReadScope| async {
                // The owner revokes while this admitted read is running.
                fixture
                    .change(crate::conversation::application::ReadGrantTransition::Revoke)
                    .await;
                Ok::<_, ()>("page")
            },
        )
        .await;
    assert_eq!(finished, Ok("page"));
    assert_eq!(
        admit
            .execute(
                &session,
                &fixture.id,
                "receiver",
                7,
                PassiveRead::RecordPage
            )
            .await,
        Err(ReadRefusal::WrongOwner)
    );
}

#[tokio::test]
async fn metadata_query_target_fences_passive_admission_and_shares() {
    use crate::conversation::application::{
        ConversationCaller, ConversationError, ShareConversation,
    };
    use crate::conversation_test_support::MemoryRepository;
    use nessa_sdk::application::agent_execution::sessions::StorageError;
    let fixture = GrantFixture::new().await;
    fixture
        .change(crate::conversation::application::ReadGrantTransition::Grant)
        .await;
    let repository = MemoryRepository::default();
    let record = fixture.store.load(&fixture.id).await.unwrap().unwrap();
    repository
        .records
        .lock()
        .unwrap()
        .insert(fixture.id.clone(), record);
    let foreign = || {
        Conversation::new(
            ConversationId::new(&Uuid::from_u128(2).to_string()).unwrap(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("owner").unwrap(),
            "historical-window".into(),
            "foreign-create".into(),
            1,
            AgentId::Claude,
            ConversationModelId::new("foreign-model").unwrap(),
            ConversationApprovalMode::Auto,
        )
        .unwrap()
    };
    let access = access();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let gateway = Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    );
    let bindings = Bindings(Mutex::new(Ok(Some(binding()))));
    let admit = AdmitPassiveRead {
        authorization: AuthorizeAction {
            access: &access,
            clock: &FixedClock,
            policy: &policy,
        },
        gateway: &gateway,
        receivers: &bindings,
        conversations: &repository,
        grants: &CONVERSATION_READ_GRANT,
        read_grants: &fixture.store,
    };
    let session = device_session().await;
    let source_reads = AtomicUsize::new(0);
    for read in [PassiveRead::RecordHead, PassiveRead::RecordPage] {
        *repository.mode_loaded_reply.lock().unwrap() = Some(foreign());
        let result = admit
            .read_with(&session, &fixture.id, "receiver", 7, read, |_| {
                source_reads.fetch_add(1, Ordering::SeqCst);
                async { Ok::<_, ()>(()) }
            })
            .await;
        assert_eq!(result, Err(ReadRefusal::Unverifiable));
        assert_eq!(source_reads.load(Ordering::SeqCst), 0);
        assert!(admit
            .execute(&session, &fixture.id, "receiver", 7, read)
            .await
            .is_ok());
    }
    let shares = ShareConversation {
        conversations: &repository,
        receivers: &bindings,
        grants: &fixture.store,
        access: &access,
    };
    let caller = ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("owner").unwrap(),
        surface_id: "desktop".into(),
        action_id: Uuid::new_v4().to_string(),
    };
    *repository.mode_loaded_reply.lock().unwrap() = Some(foreign());
    assert!(matches!(
        shares.shares(&caller, &fixture.id).await,
        Err(ConversationError::Storage(StorageError::IdentityMismatch))
    ));
    assert_eq!(shares.shares(&caller, &fixture.id).await.unwrap().len(), 1);
}
