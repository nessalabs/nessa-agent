//! Passive read admission against the local ownership adapter.

use crate::agents::domain::AgentId;
use crate::conversation::application::{
    AdmitPassiveRead, CatalogueReadError, CatalogueReadFuture, CatalogueReadOperation,
    CatalogueReadResponse, CatalogueReadScope, CatalogueReadSource, CatalogueReadValue,
    ConversationRepository, ReadCatalogue, ReadRecords, ReadRefusal, ReceiverAuthority,
    ReceiverBinding, ReceiverReadScope, RecordHead, RecordReadError, RecordReadFuture,
    RecordReadLease, RecordReadOperation, RecordReadResponse, RecordReadSource, RecordReadValue,
};
use crate::conversation::domain::{
    conversation_catalogue_stream, Conversation, ConversationApprovalMode, ConversationId,
    ConversationModelId,
};
use crate::conversation::infrastructure::LocalConversationStore;
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::application::authorization::AuthorizeAction;
use nessa_auth::application::ports::{
    AccessError, AccessReader, AccessSnapshot, Clock, CredentialEvidence, CredentialVerifier,
    PortFuture, VerifiedCredential,
};
use nessa_auth::application::session::AuthenticateSession;
use nessa_auth::domain::{
    Action, AudienceId, Credential, CredentialId, Grant, Membership, MembershipId, MembershipRole,
    MembershipStatus, OrganizationId, PrincipalId, Resource, ResourceId,
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
        };
        let calls = AtomicUsize::new(0);
        assert_eq!(
            admit
                .catalogue_with(&session, "receiver", 7, |_| {
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
