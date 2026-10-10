use super::*;
use crate::conversation::{
    application::ConversationRepository,
    domain::{Conversation, ConversationDeletion},
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::{
    agents::AgentId,
    conversation::domain::{ConversationApprovalMode, ConversationModelId},
};
use uuid::Uuid;

#[tokio::test]
async fn failed_worker_start_returns_unavailable_without_scheduler_panic() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = Arc::new(
        super::super::store::LocalConversationStore::open(&private.join("metadata.sqlite3"))
            .unwrap(),
    );
    let head = store
        .head(
            &caller().organization_id,
            &caller().principal_id,
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap();
    let result = NessaCatalogueSource::start_with_spawn(
        store.clone(),
        caller(),
        scope(&head.incarnation),
        |_run| Err(std::io::Error::other("injected thread failure")),
        || panic!("runtime builder must not run after spawn fails"),
    );
    assert!(matches!(
        result,
        Err(CatalogueWorkerError::Source(
            CatalogueSourceError::Unavailable
        ))
    ));
    let result = NessaCatalogueSource::start_with_spawn(
        store,
        caller(),
        scope(&head.incarnation),
        |run| thread::Builder::new().spawn(run),
        || Err(std::io::Error::other("injected runtime failure")),
    );
    assert!(matches!(
        result,
        Err(CatalogueWorkerError::Source(
            CatalogueSourceError::Unavailable
        ))
    ));
}

#[test]
fn source_head_finishes_when_caller_owns_the_only_blocking_slot() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let store = Arc::new(
            super::super::store::LocalConversationStore::open(&private.join("metadata.sqlite3"))
                .unwrap(),
        );
        let head = store
            .head(
                &caller().organization_id,
                &caller().principal_id,
                &crate::conversation::application::Reader::Owner,
            )
            .await
            .unwrap();
        let scope = scope(&head.incarnation);
        let mut source = NessaCatalogueSource::new(store, caller(), scope.clone()).unwrap();
        let read = tokio::task::spawn_blocking(move || source.head(&scope));
        let answer = tokio::time::timeout(std::time::Duration::from_secs(2), read)
            .await
            .expect("source read must finish with one caller blocking slot")
            .unwrap()
            .unwrap();
        assert_eq!(answer, 0);
    });
    runtime.shutdown_timeout(std::time::Duration::from_millis(20));
}

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn caller() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("alice").unwrap(),
        surface_id: "panel".into(),
        action_id: "read".into(),
    }
}
fn scope(incarnation: &str) -> Scope {
    Scope::new(
        id("receiver"),
        id("origin"),
        conversation_catalogue_stream(&caller().organization_id, &caller().principal_id),
        id(incarnation),
        conversation_catalogue_schema(),
        id("epoch"),
    )
}
fn owned(id: &ConversationId, owner: &str) -> Conversation {
    Conversation::new(
        id.clone(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new(owner).unwrap(),
        "panel".into(),
        "create".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap()
}
fn pass(scope: Scope, boundary: u64) -> CataloguePass {
    CataloguePass {
        scope,
        completed: 0,
        boundary,
        cursor: None,
        generation: 1,
    }
}

#[tokio::test]
async fn source_reads_owner_scoped_current_values_and_rejects_wrong_scope() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = Arc::new(
        super::super::store::LocalConversationStore::open(&private.join("metadata.sqlite3"))
            .unwrap(),
    );
    let alice_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let bob_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    store.create(owned(&alice_id, "alice")).await.unwrap();
    store.create(owned(&bob_id, "bob")).await.unwrap();
    // The scope's receiver reads what it was granted; the grant takes revision 2.
    crate::conversation_test_support::grant_read(store.as_ref(), &alice_id, "receiver").await;
    let head = store
        .head(
            &caller().organization_id,
            &caller().principal_id,
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap();
    let exact = scope(&head.incarnation);
    let mut bob_caller = caller();
    bob_caller.principal_id = PrincipalId::new("bob").unwrap();
    assert!(matches!(
        NessaCatalogueSource::new(store.clone(), bob_caller, exact.clone()),
        Err(CatalogueWorkerError::Source(
            CatalogueSourceError::IdentityChanged
        ))
    ));
    let mut source = NessaCatalogueSource::new(store.clone(), caller(), exact.clone()).unwrap();
    let mut blocking = source.clone();
    assert_eq!(
        tokio::task::spawn_blocking(move || blocking.head(&exact))
            .await
            .unwrap()
            .unwrap(),
        2
    );

    let request = ManifestRequest {
        pass: pass(source.scope().clone(), 2),
        max_entries: 10,
    };
    let mut blocking = source.clone();
    let page = tokio::task::spawn_blocking({
        let request = request.clone();
        move || blocking.manifest(&request)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].key.id.as_str(), alice_id.to_string());
    assert!(!page.has_more);
    let mut blocking = source.clone();
    let resolved = tokio::task::spawn_blocking({
        let request = request.clone();
        let id = page.entries[0].key.id.clone();
        move || blocking.resolve(&request.pass, &id, 1024)
    })
    .await
    .unwrap()
    .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&resolved.payload).unwrap();
    assert_eq!(json["summary"], serde_json::Value::Null);
    assert_eq!(json["id"], alice_id.to_string());
    let mut blocking = source.clone();
    let error = tokio::task::spawn_blocking({
        let request = request.clone();
        let id = page.entries[0].key.id.clone();
        move || blocking.resolve(&request.pass, &id, 1)
    })
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error, CatalogueSourceError::OversizedEntry);
    let mut blocking = source.clone();
    let invalid_payload = tokio::task::spawn_blocking({
        let request = request.clone();
        let id = page.entries[0].key.id.clone();
        move || blocking.resolve(&request.pass, &id, MAX_CATALOGUE_PAYLOAD_BYTES + 1)
    })
    .await
    .unwrap();
    assert_eq!(invalid_payload, Err(CatalogueSourceError::InvalidRequest));
    let mut blocking = source.clone();
    let invalid_page = tokio::task::spawn_blocking({
        let mut request = request.clone();
        request.max_entries = MAX_CATALOGUE_ENTRIES + 1;
        move || blocking.manifest(&request)
    })
    .await
    .unwrap();
    assert_eq!(invalid_page, Err(CatalogueSourceError::InvalidRequest));
    let wrong = Scope::new(
        id("other-receiver"),
        id("origin"),
        conversation_catalogue_stream(&caller().organization_id, &caller().principal_id),
        id(&head.incarnation),
        conversation_catalogue_schema(),
        id("epoch"),
    );
    assert_eq!(
        source.head(&wrong),
        Err(CatalogueSourceError::IdentityChanged)
    );
    let wrong_epoch = Scope::new(
        id("receiver"),
        id("origin"),
        conversation_catalogue_stream(&caller().organization_id, &caller().principal_id),
        id(&head.incarnation),
        conversation_catalogue_schema(),
        id("new-epoch"),
    );
    assert_eq!(
        source.head(&wrong_epoch),
        Err(CatalogueSourceError::IdentityChanged)
    );
    let wrong_incarnation = Scope::new(
        id("receiver"),
        id("origin"),
        conversation_catalogue_stream(&caller().organization_id, &caller().principal_id),
        id("new-incarnation"),
        conversation_catalogue_schema(),
        id("epoch"),
    );
    assert_eq!(
        source.head(&wrong_incarnation),
        Err(CatalogueSourceError::IdentityChanged)
    );
    let absent = id(&bob_id.to_string());
    let mut blocking = source.clone();
    assert_eq!(
        tokio::task::spawn_blocking({
            let pass = request.pass.clone();
            move || blocking.resolve(&pass, &absent, 1024)
        })
        .await
        .unwrap(),
        Err(CatalogueSourceError::Unavailable)
    );
}

#[tokio::test]
async fn source_resolves_deletion_after_manifest_and_keeps_newer_revision() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = Arc::new(
        super::super::store::LocalConversationStore::open(&private.join("metadata.sqlite3"))
            .unwrap(),
    );
    let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    store
        .create(owned(&conversation_id, "alice"))
        .await
        .unwrap();
    crate::conversation_test_support::grant_read(store.as_ref(), &conversation_id, "receiver")
        .await;
    let head = store
        .head(
            &caller().organization_id,
            &caller().principal_id,
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap();
    let source =
        NessaCatalogueSource::new(store.clone(), caller(), scope(&head.incarnation)).unwrap();
    let request = ManifestRequest {
        pass: pass(source.scope().clone(), head.revision),
        max_entries: 1,
    };
    let mut blocking = source.clone();
    let page = tokio::task::spawn_blocking({
        let request = request.clone();
        move || blocking.manifest(&request)
    })
    .await
    .unwrap()
    .unwrap();
    store
        .record_deletion(
            &conversation_id,
            ConversationDeletion::new(
                caller().organization_id,
                caller().principal_id,
                "panel".into(),
                "delete".into(),
                2,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let mut blocking = source.clone();
    let resolved = tokio::task::spawn_blocking({
        let pass = request.pass.clone();
        let id = page.entries[0].key.id.clone();
        move || blocking.resolve(&pass, &id, 1024)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(resolved.manifest.deleted);
    assert!(resolved.manifest.revision > page.entries[0].revision);
    assert!(resolved.payload.is_empty());
}
