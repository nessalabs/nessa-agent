//! Real metadata catalogue tests: no remote activation or native cache authority.
use super::*;
use crate::conversation::application::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, CatalogueHead, CataloguePage,
    CataloguePageRequest, CatalogueValue, ConversationAccessPolicyEvaluator, ConversationFuture,
};
use crate::conversation::{
    application::{
        AdmitOwnerPhoneReader, AdmittedCatalogueRead, CatalogueReadError, CatalogueReadOperation,
        CatalogueReadSource, CatalogueReadValue, ConversationCatalogue, ConversationRepository,
        Reader, RecordReadLease,
    },
    domain::Conversation,
    infrastructure::{LocalConversationStore, NessaCatalogueReadSource},
};
use nessa_protocol::{
    agents::AgentId,
    conversation::{
        domain::{ConversationApprovalMode, ConversationId, ConversationModelId},
        read_scope::{CatalogueReadScope, ReadRefusal},
    },
};
use nessa_sync::replication::{
    catalogue::{CataloguePass, ManifestRequest},
    domain::Id,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn admitted(
    f: &Fixture,
    reader: crate::conversation::application::OwnerPhoneReader,
) -> AdmittedCatalogueRead {
    AdmittedCatalogueRead {
        scope: CatalogueReadScope {
            receiver_id: f.receiver.receiver_id.clone(),
            organization_id: f.receiver.organization_id.clone(),
            owner_id: f.receiver.owner_id.clone(),
            access_epoch: f.receiver.access_epoch,
        },
        reader: Reader::OwnerPhone(Box::new(reader)),
    }
}
async fn phone(
    f: &Fixture,
    access_policy: Arc<dyn ConversationAccessPolicyEvaluator>,
) -> crate::conversation::application::OwnerPhoneReader {
    let actual = ActualHolding(f.receivers.clone());
    AdmitOwnerPhoneReader {
        purposes: f.registry.as_ref(),
        holding: Some(&actual),
        clock: &Time,
        gateway: &f.gateway,
        policy: Arc::new(CedarOwnerPhonePolicyEvaluator::new().unwrap()),
        access_policy: Some(access_policy),
    }
    .execute(&f.session)
    .await
    .unwrap()
    .unwrap()
}
fn metadata(f: &Fixture) -> Arc<LocalConversationStore> {
    // A canonicalized temp path is a reparse point on Windows, which the
    // private-directory check refuses. Unix resolves the temp alias first.
    #[cfg(windows)]
    let root = f._directory.path().join("metadata");
    #[cfg(not(windows))]
    let root = f._directory.path().canonicalize().unwrap().join("metadata");
    nessa_local_storage::create_directory(&root).unwrap();
    Arc::new(LocalConversationStore::open(&root.join("metadata.sqlite3")).unwrap())
}
async fn create(store: &LocalConversationStore, owner: &str, org: &str) -> ConversationId {
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    store
        .create(
            Conversation::new(
                id.clone(),
                OrganizationId::new(org).unwrap(),
                PrincipalId::new(owner).unwrap(),
                "test-surface".into(),
                id.to_string(),
                110_001,
                AgentId::Claude,
                ConversationModelId::new("model").unwrap(),
                ConversationApprovalMode::Ask,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    id
}
async fn source_head(
    source: &NessaCatalogueReadSource,
    admitted: AdmittedCatalogueRead,
) -> (nessa_sync::replication::domain::Scope, u64) {
    match source
        .read(
            admitted,
            PrincipalId::new("owner").unwrap(),
            CatalogueReadOperation::Head,
            RecordReadLease::new(()),
        )
        .await
        .unwrap()
        .value
    {
        CatalogueReadValue::Head { scope, head } => (scope, head),
        _ => panic!("head"),
    }
}
fn pass(scope: nessa_sync::replication::domain::Scope, boundary: u64) -> CataloguePass {
    CataloguePass {
        scope,
        completed: 0,
        boundary,
        cursor: None,
        generation: 1,
    }
}
#[tokio::test]
async fn owner_phone_catalogue_real_owned_current_future_rows_need_no_share_grants() {
    let f = fixture().await;
    let store = metadata(&f);
    let first = create(&store, "owner", "org").await;
    let foreign = create(&store, "other", "org").await;
    let foreignorg = create(&store, "owner", "other-org").await;
    let reader = phone(&f, crate::conversation_test_support::access_policy()).await;
    let source = NessaCatalogueReadSource::new(
        store.clone(),
        crate::conversation_test_support::access_policy(),
        id("origin"),
    );
    let (scope, head) = source_head(&source, admitted(&f, reader.clone())).await;
    let request = ManifestRequest {
        pass: pass(scope.clone(), head),
        max_entries: 16,
    };
    let page = match source
        .read(
            admitted(&f, reader.clone()),
            f.session.context().principal_id().clone(),
            CatalogueReadOperation::Manifest(request),
            RecordReadLease::new(()),
        )
        .await
        .unwrap()
        .value
    {
        CatalogueReadValue::Manifest(page) => page,
        _ => panic!("manifest"),
    };
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].key.id.as_str(), first.to_string());
    assert!(!page.has_more);
    for forbidden in [foreign, foreignorg] {
        assert!(store
            .resolve(
                &f.receiver.organization_id,
                &f.receiver.owner_id,
                &Reader::OwnerPhone(Box::new(reader.clone())),
                scope.incarnation().as_str(),
                &forbidden
            )
            .await
            .unwrap()
            .is_none());
    }
    let paired = Reader::PairedDevice {
        receiver_id: f.receiver.receiver_id.clone(),
    };
    assert_eq!(
        store
            .head(&f.receiver.organization_id, &f.receiver.owner_id, &paired)
            .await
            .unwrap()
            .revision,
        0
    );
    assert!(store
        .resolve(
            &f.receiver.organization_id,
            &f.receiver.owner_id,
            &paired,
            scope.incarnation().as_str(),
            &first
        )
        .await
        .unwrap()
        .is_none());
    let future = create(&store, "owner", "org").await;
    let (nextscope, nexthead) = source_head(&source, admitted(&f, reader.clone())).await;
    assert!(nexthead > head);
    let request = ManifestRequest {
        pass: pass(nextscope.clone(), nexthead),
        max_entries: 16,
    };
    let page = match source
        .read(
            admitted(&f, reader.clone()),
            f.session.context().principal_id().clone(),
            CatalogueReadOperation::Manifest(request),
            RecordReadLease::new(()),
        )
        .await
        .unwrap()
        .value
    {
        CatalogueReadValue::Manifest(page) => page,
        _ => panic!("manifest"),
    };
    assert_eq!(page.entries.len(), 2);
    assert!(page
        .entries
        .iter()
        .any(|entry| entry.key.id.as_str() == future.to_string()));
    let descriptor = page.entries[0].clone();
    assert!(matches!(
        source
            .read(
                admitted(&f, reader),
                f.session.context().principal_id().clone(),
                CatalogueReadOperation::Resolve {
                    pass: page.request.pass,
                    descriptor,
                    max_payload_bytes: 4096
                },
                RecordReadLease::new(())
            )
            .await
            .unwrap()
            .value,
        CatalogueReadValue::Resolve(_)
    ));
    source.shutdown().await.unwrap();
}
struct DenyLater {
    real: crate::conversation::infrastructure::CedarConversationAccessPolicy,
    enabled: AtomicBool,
    calls: AtomicUsize,
    errors: AtomicBool,
}
impl ConversationAccessPolicyEvaluator for DenyLater {
    fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
        if self.enabled.load(Ordering::SeqCst)
            && ask.action == AccessAction::Locate
            && self.calls.fetch_add(1, Ordering::SeqCst) >= 1
        {
            return AccessDecision::Deny(if self.errors.load(Ordering::SeqCst) {
                AccessDenial::Unavailable
            } else {
                AccessDenial::Forbidden {
                    policies: vec!["test.later-candidate".into()],
                }
            });
        }
        self.real.evaluate(ask)
    }
}
#[tokio::test]
async fn owner_phone_catalogue_later_denial_returns_no_partial_page_or_new_cursor() {
    let f = fixture().await;
    let store = metadata(&f);
    create(&store, "owner", "org").await;
    let denied = create(&store, "owner", "org").await;
    store
        .record_deletion(
            &denied,
            crate::conversation::domain::ConversationDeletion::new(
                f.receiver.organization_id.clone(),
                f.receiver.owner_id.clone(),
                "test-surface".into(),
                "delete-later".into(),
                110_002,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let policy = Arc::new(DenyLater {
        real: crate::conversation::infrastructure::CedarConversationAccessPolicy::new().unwrap(),
        enabled: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        errors: AtomicBool::new(false),
    });
    let reader = phone(&f, policy.clone()).await;
    let source = NessaCatalogueReadSource::new(store, policy.clone(), id("origin"));
    let (scope, head) = source_head(&source, admitted(&f, reader.clone())).await;
    let checkpoint = pass(scope, head);
    let request = ManifestRequest {
        pass: checkpoint.clone(),
        max_entries: 16,
    };
    policy.enabled.store(true, Ordering::SeqCst);
    for unavailable in [false, false, true] {
        policy.errors.store(unavailable, Ordering::SeqCst);
        policy.calls.store(0, Ordering::SeqCst);
        let result = source
            .read(
                admitted(&f, reader.clone()),
                f.session.context().principal_id().clone(),
                CatalogueReadOperation::Manifest(request.clone()),
                RecordReadLease::new(()),
            )
            .await;
        assert!(matches!(
            &result,
            Err(CatalogueReadError::SourceUnavailable)
        ));
        assert_eq!(
            policy.calls.load(Ordering::SeqCst),
            2,
            "two distinct real live/tombstone candidate facts; no retry scan"
        );
        assert_eq!(request.pass, checkpoint);
        // Refusal has no manifest payload or denied-ID cursor to disclose.
        let error = match result {
            Err(error) => format!("{error:?}"),
            Ok(_) => unreachable!(),
        };
        assert!(!error.contains(&denied.to_string()));
    }
    source.shutdown().await.unwrap();
}
struct CountedCatalogue {
    store: Arc<LocalConversationStore>,
    calls: AtomicUsize,
}
impl ConversationCatalogue for CountedCatalogue {
    fn head(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        reader: &Reader,
    ) -> ConversationFuture<'_, CatalogueHead> {
        let organization = organization.clone();
        let owner = owner.clone();
        let reader = reader.clone();
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.store.head(&organization, &owner, &reader).await
        })
    }
    fn page(&self, request: CataloguePageRequest) -> ConversationFuture<'_, CataloguePage> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.store.page(request).await
        })
    }
    fn resolve(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        reader: &Reader,
        incarnation: &str,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<CatalogueValue>> {
        let organization = organization.clone();
        let owner = owner.clone();
        let reader = reader.clone();
        let incarnation = incarnation.to_owned();
        let id = id.clone();
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.store
                .resolve(&organization, &owner, &reader, &incarnation, &id)
                .await
        })
    }
}
#[tokio::test]
async fn owner_phone_catalogue_reader_scope_contradictions_refuse_before_metadata() {
    let f = fixture().await;
    let store = metadata(&f);
    let reader = phone(&f, crate::conversation_test_support::access_policy()).await;
    let counted = Arc::new(CountedCatalogue {
        store,
        calls: AtomicUsize::new(0),
    });
    let source = NessaCatalogueReadSource::new(
        counted.clone(),
        crate::conversation_test_support::access_policy(),
        id("origin"),
    );
    for field in 0..4 {
        let mut input = admitted(&f, reader.clone());
        match field {
            0 => input.scope.receiver_id = "foreign".into(),
            1 => input.scope.owner_id = PrincipalId::new("foreign").unwrap(),
            2 => input.scope.organization_id = OrganizationId::new("foreign").unwrap(),
            _ => input.scope.access_epoch += 1,
        };
        assert!(matches!(
            source
                .read(
                    input,
                    f.session.context().principal_id().clone(),
                    CatalogueReadOperation::Head,
                    RecordReadLease::new(())
                )
                .await,
            Err(CatalogueReadError::Admission(ReadRefusal::Unverifiable))
        ));
    }
    assert_eq!(counted.calls.load(Ordering::SeqCst), 0);
    source.shutdown().await.unwrap();
}
