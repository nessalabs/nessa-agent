use super::super::store::LocalConversationStore;
use super::*;
use crate::conversation::{
    application::{
        AccessAction, AccessAsk, AccessDecision, AccessDenial, CatalogueEntryAccess, CatalogueHead,
        CataloguePageRequest, ConversationAccessPolicyEvaluator, ConversationCatalogue,
        ConversationRepository, Reader,
    },
    domain::{
        Capabilities, Conversation, ConversationDeletion, ShareAccess, ToolPolicy, ToolPreset,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::{
    agents::AgentId,
    conversation::domain::{ConversationApprovalMode, ConversationModelId},
};
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

#[tokio::test]
async fn failed_worker_start_returns_unavailable_without_scheduler_panic() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = Arc::new(
        super::super::store::LocalConversationStore::open(&private.join("metadata.sqlite3"))
            .unwrap()
            .with_access_policy(crate::conversation_test_support::access_policy()),
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
        crate::conversation_test_support::catalogue_entries(caller().principal_id.as_str()),
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
        crate::conversation_test_support::catalogue_entries(caller().principal_id.as_str()),
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
                .unwrap()
                .with_access_policy(crate::conversation_test_support::access_policy()),
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
        let mut source = NessaCatalogueSource::new(
            store,
            caller(),
            crate::conversation_test_support::catalogue_entries(caller().principal_id.as_str()),
            scope.clone(),
        )
        .unwrap();
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
            .unwrap()
            .with_access_policy(crate::conversation_test_support::access_policy()),
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
        NessaCatalogueSource::new(
            store.clone(),
            bob_caller,
            crate::conversation_test_support::catalogue_entries(caller().principal_id.as_str()),
            exact.clone()
        ),
        Err(CatalogueWorkerError::Source(
            CatalogueSourceError::IdentityChanged
        ))
    ));
    let mut source = NessaCatalogueSource::new(
        store.clone(),
        caller(),
        crate::conversation_test_support::catalogue_entries(caller().principal_id.as_str()),
        exact.clone(),
    )
    .unwrap();
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
            .unwrap()
            .with_access_policy(crate::conversation_test_support::access_policy()),
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
    let source = NessaCatalogueSource::new(
        store.clone(),
        caller(),
        crate::conversation_test_support::catalogue_entries(caller().principal_id.as_str()),
        scope(&head.incarnation),
    )
    .unwrap();
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

/// The checked-in access policy, except that it refuses `catalogue.entry`
/// for a grant of `refused`, or decides nothing when `undecided`; it counts
/// the entries it is asked about.
struct EntryPolicy {
    refused: Option<Capabilities>,
    undecided: bool,
    asked: AtomicUsize,
}
impl EntryPolicy {
    fn new(refused: Option<Capabilities>, undecided: bool) -> Arc<Self> {
        Arc::new(Self {
            refused,
            undecided,
            asked: AtomicUsize::new(0),
        })
    }
    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}
impl ConversationAccessPolicyEvaluator for EntryPolicy {
    fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
        if ask.action == AccessAction::CatalogueEntry {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if self.undecided {
                return AccessDecision::Deny(AccessDenial::Unavailable);
            }
            if self.refused.is_some() && ask.held == self.refused {
                return AccessDecision::Deny(AccessDenial::NotPermitted);
            }
        }
        crate::conversation_test_support::access_policy_ref().evaluate(ask)
    }
}

/// Alice's conversations, created in order and granted to "receiver" with
/// the roles given, and the receiver's head over them.
async fn granted_catalogue(
    roles: &[Capabilities],
) -> (
    tempfile::TempDir,
    Arc<LocalConversationStore>,
    Vec<ConversationId>,
    CatalogueHead,
) {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = Arc::new(
        LocalConversationStore::open(&private.join("metadata.sqlite3"))
            .unwrap()
            .with_access_policy(crate::conversation_test_support::access_policy()),
    );
    let mut ids = Vec::new();
    for role in roles {
        let conversation_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        store
            .create(owned(&conversation_id, "alice"))
            .await
            .unwrap();
        let policy =
            (*role == Capabilities::drive()).then_some(ToolPolicy::preset(ToolPreset::NoShell));
        crate::conversation_test_support::grant_as(
            store.as_ref(),
            &conversation_id,
            "receiver",
            ShareAccess::new(*role, policy).unwrap(),
        )
        .await;
        ids.push(conversation_id);
    }
    let head = store
        .head(
            &caller().organization_id,
            &caller().principal_id,
            &Reader::PairedDevice {
                receiver_id: "receiver".into(),
            },
        )
        .await
        .unwrap();
    (directory, store, ids, head)
}

fn admitted_by(
    store: &Arc<LocalConversationStore>,
    policy: Arc<EntryPolicy>,
    incarnation: &str,
) -> NessaCatalogueSource {
    NessaCatalogueSource::new(
        store.clone(),
        caller(),
        CatalogueEntryAccess::new(policy, caller().principal_id),
        scope(incarnation),
    )
    .unwrap()
}

async fn manifest(
    source: &NessaCatalogueSource,
    request: &ManifestRequest,
) -> Result<ManifestPage, CatalogueSourceError> {
    let mut blocking = source.clone();
    let request = request.clone();
    tokio::task::spawn_blocking(move || blocking.manifest(&request))
        .await
        .unwrap()
}

async fn resolve(
    source: &NessaCatalogueSource,
    pass: &CataloguePass,
    id: &ConversationId,
) -> Result<ResolvedEntry, CatalogueSourceError> {
    let mut blocking = source.clone();
    let (pass, id) = (pass.clone(), Id::new(id.to_string()).unwrap());
    tokio::task::spawn_blocking(move || blocking.resolve(&pass, &id, 4096))
        .await
        .unwrap()
}

/// The store returns, with each row a paired reader's page or resolve
/// finds, the role its grant holds; the owner's rows hold none.
#[tokio::test]
async fn a_catalogue_row_carries_the_role_its_reader_holds() {
    let roles = [
        Capabilities::read(),
        Capabilities::comment(),
        Capabilities::drive(),
    ];
    let (_directory, store, ids, head) = granted_catalogue(&roles).await;
    let request = |reader: Reader| CataloguePageRequest {
        organization: caller().organization_id,
        owner: caller().principal_id,
        reader,
        manifest: ManifestRequest {
            pass: pass(scope(&head.incarnation), head.revision),
            max_entries: 8,
        },
    };
    let paired = Reader::PairedDevice {
        receiver_id: "receiver".into(),
    };
    let page = store.page(request(paired.clone())).await.unwrap();
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.held)
            .collect::<Vec<_>>(),
        roles.map(Some)
    );
    let owners = store.page(request(Reader::Owner)).await.unwrap();
    assert!(owners.entries.iter().all(|entry| entry.held.is_none()));
    let resolved = store
        .resolve(
            &caller().organization_id,
            &caller().principal_id,
            &paired,
            &head.incarnation,
            &ids[1],
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.descriptor.held, Some(Capabilities::comment()));
}

/// A manifest admits each entry the grant index returns: an entry the
/// policy refuses is left out, and the index is read on past it so the page
/// is still full and its continuation is its last entry. Each kind of entry
/// is asked about once per page.
#[tokio::test]
async fn a_manifest_leaves_out_what_the_policy_refuses_and_reads_on_to_fill_the_page() {
    let roles = [
        Capabilities::read(),
        Capabilities::read(),
        Capabilities::comment(),
        Capabilities::read(),
        Capabilities::read(),
        Capabilities::read(),
        Capabilities::comment(),
        Capabilities::read(),
    ];
    let (_directory, store, ids, head) = granted_catalogue(&roles).await;
    let policy = EntryPolicy::new(Some(Capabilities::read()), false);
    let source = admitted_by(&store, policy.clone(), &head.incarnation);
    let request = ManifestRequest {
        pass: pass(source.scope().clone(), head.revision),
        max_entries: 2,
    };
    let page = manifest(&source, &request).await.unwrap();
    let shown: Vec<String> = page
        .entries
        .iter()
        .map(|entry| entry.key.id.as_str().to_owned())
        .collect();
    assert_eq!(shown, [ids[2].to_string(), ids[6].to_string()]);
    assert!(page.has_more, "a refused row is still behind the page");
    assert_eq!(policy.asked(), 2, "one Read and one Comment entry asked");
    nessa_sync::replication::catalogue::validate_manifest(&request, &page, MAX_CATALOGUE_ENTRIES)
        .unwrap();
    // The next page starts after the last entry shown and finds only
    // refused rows: it is empty and final, never "more" with nothing in it.
    let next = ManifestRequest {
        pass: CataloguePass {
            cursor: page.entries.last().map(|entry| entry.key.clone()),
            ..request.pass.clone()
        },
        ..request.clone()
    };
    let last = manifest(&source, &next).await.unwrap();
    assert!(last.entries.is_empty());
    assert!(!last.has_more);
    nessa_sync::replication::catalogue::validate_manifest(&next, &last, MAX_CATALOGUE_ENTRIES)
        .unwrap();
    // The checked-in policy shows every granted row.
    let everything = admitted_by(&store, EntryPolicy::new(None, false), &head.incarnation);
    let all = ManifestRequest {
        max_entries: 16,
        ..request.clone()
    };
    assert_eq!(
        manifest(&everything, &all).await.unwrap().entries.len(),
        roles.len()
    );
}

/// A resolve of an entry the policy refuses is missing, as an ungranted one
/// is; one it admits resolves.
#[tokio::test]
async fn a_resolve_the_policy_refuses_is_missing() {
    let (_directory, store, ids, head) =
        granted_catalogue(&[Capabilities::read(), Capabilities::comment()]).await;
    let source = admitted_by(
        &store,
        EntryPolicy::new(Some(Capabilities::read()), false),
        &head.incarnation,
    );
    let pass = pass(source.scope().clone(), head.revision);
    assert_eq!(
        resolve(&source, &pass, &ids[0]).await,
        Err(CatalogueSourceError::Unavailable)
    );
    let resolved = resolve(&source, &pass, &ids[1]).await.unwrap();
    assert_eq!(resolved.manifest.key.id.as_str(), ids[1].to_string());
}

/// A policy that cannot decide makes a manifest and a resolve unavailable,
/// never shorter or missing.
#[tokio::test]
async fn a_catalogue_read_the_policy_cannot_decide_is_unavailable() {
    let (_directory, store, ids, head) = granted_catalogue(&[Capabilities::read()]).await;
    let source = admitted_by(&store, EntryPolicy::new(None, true), &head.incarnation);
    let request = ManifestRequest {
        pass: pass(source.scope().clone(), head.revision),
        max_entries: 4,
    };
    assert_eq!(
        manifest(&source, &request).await,
        Err(CatalogueSourceError::Unavailable)
    );
    assert_eq!(
        resolve(&source, &request.pass, &ids[0]).await,
        Err(CatalogueSourceError::Unavailable)
    );
}
