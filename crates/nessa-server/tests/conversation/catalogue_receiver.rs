//! Two independent process-local receiver files exercise the real sync core.
//! This fixture uses Nessa's SQLite version, avoiding a second rusqlite in the
//! Cargo graph. It is receiver test data, never a production metadata mirror.
use super::*;
use crate::{
    agents::domain::AgentId,
    conversation::{
        application::{ConversationRepository, ConversationSummaries},
        domain::{
            Conversation, ConversationApprovalMode, ConversationDeletion, ConversationId,
            ConversationModelId, ConversationSummary,
        },
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sync::replication::{
    catalogue::{
        apply_next_page, begin_or_resume, CataloguePagePlan, CatalogueStore, CatalogueStoreError,
    },
    infrastructure::MemoryAuthorizer,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

fn sid(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn owner() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("alice").unwrap(),
        surface_id: "panel".into(),
        action_id: "sync".into(),
    }
}
fn scope(receiver: &str, incarnation: &str) -> Scope {
    Scope::new(
        sid(receiver),
        sid("origin"),
        conversation_catalogue_stream(&owner().organization_id, &owner().principal_id),
        sid(incarnation),
        conversation_catalogue_schema(),
        sid("epoch-1"),
    )
}
fn conversation(id: ConversationId, owner: &str) -> Conversation {
    Conversation::new(
        id,
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
use crate::catalogue_receiver_store::SqliteReceiver;

const CHILD: &str =
    "conversation::infrastructure::catalogue_source::receiver_tests::receiver_child";

#[tokio::test]
async fn receiver_child() {
    let Ok(source_path) = std::env::var("NESSA_259_CHILD_SOURCE") else {
        return;
    };
    let cache_path = PathBuf::from(std::env::var("NESSA_259_CHILD_CACHE").unwrap());
    let receiver = std::env::var("NESSA_259_CHILD_RECEIVER").unwrap();
    let store = Arc::new(
        super::super::store::LocalConversationStore::open(Path::new(&source_path)).unwrap(),
    );
    let head = store
        .head(&owner().organization_id, &owner().principal_id)
        .await
        .unwrap();
    let scope = scope(&receiver, &head.incarnation);
    let source = NessaCatalogueSource::new(store, owner(), scope.clone()).unwrap();
    let stop_after_page = std::env::var_os("NESSA_259_CHILD_STOP_AFTER_PAGE").is_some();
    tokio::task::spawn_blocking(move || {
        let mut cache = SqliteReceiver::open(&cache_path);
        let mut source = source;
        let mut authorizer = MemoryAuthorizer::allowed(scope.clone());
        while let Some(mut pass) =
            begin_or_resume(&scope, &mut authorizer, &mut source, &mut cache).unwrap()
        {
            loop {
                let progress = apply_next_page(
                    &pass,
                    37,
                    128 * 1024,
                    &mut authorizer,
                    &mut source,
                    &mut cache,
                )
                .unwrap();
                if stop_after_page {
                    return;
                }
                match progress.active {
                    Some(next) => pass = next,
                    None => break,
                }
            }
        }
    })
    .await
    .unwrap();
}

fn child(source: &Path, cache: &Path, receiver: &str, stop_after_page: bool) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg(CHILD)
        .arg("--nocapture")
        .env("NESSA_259_CHILD_SOURCE", source)
        .env("NESSA_259_CHILD_CACHE", cache)
        .env("NESSA_259_CHILD_RECEIVER", receiver);
    if stop_after_page {
        command.env("NESSA_259_CHILD_STOP_AFTER_PAGE", "1");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[tokio::test]
async fn two_receiver_processes_reopen_independent_durable_progress() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let source_path = private.join("metadata.sqlite3");
    let store = super::super::store::LocalConversationStore::open(&source_path).unwrap();
    let mut first = None;
    for index in 0..620 {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        if index == 0 {
            first = Some(id.clone());
        }
        let owner = if index < 600 { "alice" } else { "bob" };
        store.create(conversation(id, owner)).await.unwrap();
    }
    let left = directory.path().join("left.sqlite3");
    let right = directory.path().join("right.sqlite3");
    child(&source_path, &left, "receiver-left", true);
    assert_eq!(SqliteReceiver::open(&left).count(), 37);
    let mut paused_cache = SqliteReceiver::open(&left);
    let saved_pass = SqliteReceiver::snapshot(&paused_cache.conn)
        .unwrap()
        .unwrap()
        .active
        .unwrap();
    let delayed_pass = CataloguePass {
        cursor: None,
        ..saved_pass.clone()
    };
    let delayed_request = ManifestRequest {
        pass: delayed_pass.clone(),
        max_entries: 37,
    };
    assert_eq!(
        paused_cache.apply_page(
            CataloguePagePlan::new(
                ManifestPage {
                    request: delayed_request,
                    entries: vec![],
                    has_more: false,
                },
                vec![],
                vec![],
            )
            .unwrap()
        ),
        Err(CatalogueStoreError::Stale)
    );
    assert_eq!(paused_cache.count(), 37);
    child(&source_path, &right, "receiver-right", false);
    assert_eq!(SqliteReceiver::open(&right).count(), 600);
    let first = first.unwrap();
    store
        .record(
            &first,
            ConversationSummary::after_message(None, "changed while passing", None, 2),
        )
        .await
        .unwrap();
    let new_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    store
        .create(conversation(new_id.clone(), "alice"))
        .await
        .unwrap();
    child(&source_path, &left, "receiver-left", false);
    assert_eq!(SqliteReceiver::open(&left).count(), 601);
    assert_eq!(
        SqliteReceiver::open(&left).payload(&first)["summary"]["preview"],
        "changed while passing"
    );
    assert_eq!(SqliteReceiver::open(&right).count(), 600);
    child(&source_path, &right, "receiver-right", false);
    assert_eq!(SqliteReceiver::open(&right).count(), 601);
    store
        .record_deletion(
            &first,
            ConversationDeletion::new(
                owner().organization_id,
                owner().principal_id,
                "panel".into(),
                "delete".into(),
                3,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    child(&source_path, &left, "receiver-left", false);
    let mut cache = SqliteReceiver::open(&left);
    let saved = SqliteReceiver::snapshot(&cache.conn).unwrap().unwrap();
    let changed = Scope::new(
        saved.scope.receiver().clone(),
        saved.scope.origin().clone(),
        saved.scope.stream().clone(),
        saved.scope.incarnation().clone(),
        saved.scope.schema().clone(),
        sid("epoch-2"),
    );
    let reset = cache.reset(&changed, saved).unwrap();
    assert_eq!(reset.completed, 0);
    assert_eq!(cache.count(), 1);
    assert_eq!(
        cache
            .cached_revision(&changed, &sid(&new_id.to_string()))
            .unwrap(),
        None
    );
    assert!(cache
        .cached_revision(&changed, &sid(&first.to_string()))
        .unwrap()
        .is_some());
}
