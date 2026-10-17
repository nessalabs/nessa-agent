//! Ownership, tombstones and summaries in one database: create-once
//! ownership, tombstones and summaries that cannot stand without their record,
//! rows refused rather than repaired, and a list that reads only its owner's.
use super::store::{list_query, LocalConversationStore, UNFINISHED};
use crate::conversation::infrastructure::MAX_CATALOGUE_CHANGE_WATCHES;
use crate::conversation::{
    application::{
        CatalogueChangeWatch, CatalogueKey, CataloguePageRequest, CatalogueWatchError,
        CatalogueWatchState, ConversationCatalogue, ConversationCreationDisposition,
        ConversationError, ConversationListing, ConversationModeApplication,
        ConversationModeRequest, ConversationModeRequestState, ConversationRepository,
        ConversationSummaries, Reader, ShareGrantTransition, ShareGrants, WatchCatalogue,
    },
    domain::{
        Capabilities, Conversation, ConversationDeletion, ProviderSessionErasure,
        ProviderSessionLink, ShareAccess, ToolPolicy, ToolPreset, ToolRules,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId, MAX_IDENTIFIER_BYTES};
use nessa_local_database::rusqlite::{params, Connection, StatementStatus};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::conversation_catalogue_schema;
use nessa_protocol::conversation::domain::{
    conversation_catalogue_stream, ConversationApprovalMode, ConversationId, ConversationModelId,
    ConversationPreview, ConversationSummary, ConversationTitle,
};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use nessa_sync::replication::{
    catalogue::{CataloguePass, EntryKey, ManifestRequest, MAX_CATALOGUE_ENTRIES},
    domain::{Id, Scope},
};
use std::path::{Path, PathBuf};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};
use uuid::Uuid;

struct Opened {
    _directory: tempfile::TempDir,
    path: PathBuf,
    store: LocalConversationStore,
}
fn opened() -> Opened {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let store = LocalConversationStore::open(&path)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    Opened {
        _directory: directory,
        path,
        store,
    }
}
/// A second connection, for what the store would never write itself.
fn raw(path: &Path) -> Connection {
    Connection::open(path).unwrap()
}
fn new_id() -> ConversationId {
    ConversationId::new(&Uuid::new_v4().to_string()).unwrap()
}
fn owned_by(id: &ConversationId, organization: &str, owner: &str, at: u64) -> Conversation {
    Conversation::new(
        id.clone(),
        OrganizationId::new(organization).unwrap(),
        PrincipalId::new(owner).unwrap(),
        "panel".into(),
        "create".into(),
        at,
        AgentId::Claude,
        ConversationModelId::new("test-model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap()
}
fn owned(id: &ConversationId) -> Conversation {
    owned_by(id, "org", "alice", 1)
}

#[tokio::test]
async fn mode_intent_and_commit_survive_restart_without_reapplying() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let request = ConversationModeRequest {
        conversation_id: id.clone(),
        organization_id: OrganizationId::new("org").unwrap(),
        request_id: "mode-1".into(),
        initiator_principal_id: PrincipalId::new("alice").unwrap(),
        initiator_surface_id: "panel".into(),
        prior: ConversationApprovalMode::Ask,
        requested: ConversationApprovalMode::Auto,
        state: ConversationModeRequestState::Pending,
        application: None,
        requested_at_ms: 2,
    };
    opened
        .store
        .begin_mode_change(request.clone())
        .await
        .unwrap();
    assert_eq!(
        opened.store.pending_mode_change(&id).await.unwrap(),
        Some(request.clone())
    );
    assert!(matches!(
        opened
            .store
            .begin_mode_change(ConversationModeRequest {
                request_id: "mode-2".into(),
                ..request.clone()
            })
            .await,
        Err(ConversationError::ApprovalModeUncertain)
    ));
    let observed = opened
        .store
        .observe_mode_application(&id, "mode-1", ConversationModeApplication::Applied)
        .await
        .unwrap();
    assert_eq!(
        observed.application,
        Some(ConversationModeApplication::Applied)
    );
    opened
        .store
        .finish_mode_change(&id, "mode-1", ConversationModeRequestState::Applied)
        .await
        .unwrap();
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        2
    );
    drop(opened.store);
    let reopened = LocalConversationStore::open(&opened.path)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    assert_eq!(reopened.pending_mode_change(&id).await.unwrap(), None);
    assert_eq!(
        ConversationRepository::load(&reopened, &id)
            .await
            .unwrap()
            .unwrap()
            .approval_mode(),
        ConversationApprovalMode::Auto
    );
    assert_eq!(
        reopened
            .mode_change(&id, "mode-1")
            .await
            .unwrap()
            .unwrap()
            .state,
        ConversationModeRequestState::Applied
    );
    assert_eq!(
        reopened
            .finish_mode_change(&id, "mode-1", ConversationModeRequestState::Applied)
            .await
            .unwrap()
            .state,
        ConversationModeRequestState::Applied
    );
    assert_eq!(
        reopened
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        2
    );
}

#[tokio::test]
async fn an_applied_mode_request_without_application_evidence_is_unreadable() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let connection = raw(&opened.path);
    connection
        .execute(
            "INSERT INTO mode_requests
             (conversation_id, request_id, initiator, surface, prior_mode,
              requested_mode, state, application, requested_at_ms)
             VALUES (?1, 'bad-result', 'alice', 'panel', 'ask', 'auto', 'applied', 'refused', 2)",
            [id.to_string()],
        )
        .unwrap();
    assert!(matches!(
        opened.store.mode_change(&id, "bad-result").await,
        Err(ConversationError::Metadata)
    ));
}

#[test]
fn an_alpha_v1_database_is_refused_without_migrating_its_rows() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let old = nessa_local_database::Schema::new(
        "CREATE TABLE conversations (id TEXT PRIMARY KEY NOT NULL) STRICT;\nPRAGMA user_version = 1;",
    )
    .unwrap();
    let connection = nessa_local_database::open(&path, &old).unwrap();
    connection
        .execute("INSERT INTO conversations (id) VALUES ('old-chat')", [])
        .unwrap();
    drop(connection);

    assert!(matches!(
        LocalConversationStore::open(&path),
        Err(nessa_local_database::OpenError::Version {
            found: 1,
            expected: 6
        })
    ));
    let old_row: String = raw(&path)
        .query_row("SELECT id FROM conversations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(old_row, "old-chat");
}

#[test]
fn an_alpha_v2_database_is_refused_without_erasing_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let old = nessa_local_database::Schema::new(
        "CREATE TABLE conversations (id TEXT PRIMARY KEY NOT NULL) STRICT;\nPRAGMA user_version = 2;",
    ).unwrap();
    let connection = nessa_local_database::open(&path, &old).unwrap();
    connection
        .execute("INSERT INTO conversations (id) VALUES ('retained')", [])
        .unwrap();
    drop(connection);
    assert!(matches!(
        LocalConversationStore::open(&path),
        Err(nessa_local_database::OpenError::Version {
            found: 2,
            expected: 6
        })
    ));
    let retained: String = raw(&path)
        .query_row("SELECT id FROM conversations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(retained, "retained");
}

fn deletion(request: &str) -> ConversationDeletion {
    ConversationDeletion::new(
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("alice").unwrap(),
        "panel".into(),
        request.into(),
        50,
    )
    .unwrap()
}
fn said(text: &str, at: u64) -> ConversationSummary {
    ConversationSummary::after_message(None, text, None, at)
}
fn org() -> OrganizationId {
    OrganizationId::new("org").unwrap()
}
fn alice() -> PrincipalId {
    PrincipalId::new("alice").unwrap()
}
fn catalogue_request(
    owner: PrincipalId,
    incarnation: &str,
    completed: u64,
    boundary: u64,
    after: Option<CatalogueKey>,
    limit: usize,
) -> CataloguePageRequest {
    let scope = Scope::new(
        Id::new("metadata-test-receiver").unwrap(),
        Id::new("metadata-test-origin").unwrap(),
        conversation_catalogue_stream(&org(), &owner),
        Id::new(incarnation).unwrap(),
        conversation_catalogue_schema(),
        Id::new("epoch-1").unwrap(),
    );
    let manifest = ManifestRequest {
        pass: CataloguePass {
            scope,
            completed,
            boundary,
            cursor: after.map(|key| EntryKey {
                creation: key.creation,
                id: Id::new(key.id.to_string()).unwrap(),
            }),
            generation: 1,
        },
        max_entries: limit,
    };
    CataloguePageRequest {
        organization: org(),
        owner,
        reader: Reader::Owner,
        manifest,
    }
}
async fn listed(
    store: &LocalConversationStore,
    owner: &PrincipalId,
    archived: bool,
    limit: usize,
) -> (Vec<ConversationId>, usize) {
    let listed = ConversationListing::list(store, &org(), owner, archived, limit)
        .await
        .unwrap();
    (
        listed
            .conversations
            .into_iter()
            .map(|listed| listed.conversation.id().clone())
            .collect(),
        listed.unreadable,
    )
}

#[tokio::test]
async fn ownership_is_create_once_and_survives_reopening() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let id = new_id();
    let original = owned(&id);
    let created = store.create(original.clone()).await.unwrap();
    assert_eq!(created.conversation, original);
    assert_eq!(
        created.disposition,
        ConversationCreationDisposition::Created
    );
    let impostor = Conversation::new(
        id.clone(),
        org(),
        PrincipalId::new("bob").unwrap(),
        "other".into(),
        "overwrite".into(),
        456,
        AgentId::Codex,
        ConversationModelId::new("test-model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap();
    let existing = store.create(impostor).await.unwrap();
    assert_eq!(existing.conversation, original);
    assert_eq!(
        existing.disposition,
        ConversationCreationDisposition::Existing
    );
    drop(store);
    let store = LocalConversationStore::open(&path)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    let loaded = ConversationRepository::load(&store, &id).await.unwrap();
    assert_eq!(loaded, Some(original));
    assert_eq!(loaded.unwrap().agent(), Some(AgentId::Claude));
    assert_eq!(
        ConversationRepository::load(&store, &new_id())
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_record_naming_an_agent_this_build_cannot_start_is_read_with_no_agent_and_can_be_deleted()
{
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    raw(&path)
        .execute(
            "UPDATE conversations SET agent = 'gemini' WHERE id = ?1",
            [id.to_string()],
        )
        .unwrap();
    // Read, with no agent: never taken for another agent's conversation, and
    // never "storage is unavailable" — the row was read without trouble.
    let loaded = ConversationRepository::load(&store, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.agent(), None);
    // Listed, and deletable: neither needs the agent.
    ConversationSummaries::record(&store, &id, said("hello", 5))
        .await
        .unwrap();
    assert_eq!(
        listed(&store, &alice(), false, 10).await,
        (vec![id.clone()], 0)
    );
    let deleted = store
        .record_deletion(&id, deletion("delete"))
        .await
        .unwrap();
    assert!(deleted.deletion().is_some());
    assert_eq!(
        ConversationRepository::load(&store, &id)
            .await
            .unwrap()
            .unwrap(),
        deleted
    );
    // Creating it again finds it; it is never written as some agent's record.
    assert!(matches!(
        store.create(owned(&id)).await,
        Ok(created) if created.conversation == deleted
    ));
}

#[tokio::test]
async fn a_conversation_with_no_agent_is_never_created() {
    let Opened {
        _directory, store, ..
    } = opened();
    let id = new_id();
    let unknown = Conversation::restore(
        id.clone(),
        org(),
        alice(),
        "panel".into(),
        "create".into(),
        1,
        None,
        ConversationModelId::new("test-model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap();
    assert!(matches!(
        store.create(unknown).await,
        Err(ConversationError::AgentUnsupported)
    ));
    assert_eq!(
        ConversationRepository::load(&store, &id).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn a_tombstone_outlives_reopening_and_its_identity_is_never_created_again() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    // Nothing to delete without a record.
    assert!(matches!(
        store.record_deletion(&new_id(), deletion("delete-0")).await,
        Err(ConversationError::NotFound)
    ));
    let first = store
        .record_deletion(&id, deletion("delete-1"))
        .await
        .unwrap();
    assert_eq!(first.deletion(), Some(&deletion("delete-1")));
    // A later request's delete does not replace the decision; the same
    // decision fills in what it read of the history, once.
    let later = store
        .record_deletion(&id, deletion("delete-2"))
        .await
        .unwrap();
    assert_eq!(later.deletion(), Some(&deletion("delete-1")));
    let session = ExecutionSessionId::new("provider-session").unwrap();
    let read = deletion("delete-1").after_reading(Some(session.clone()));
    store.record_deletion(&id, read.clone()).await.unwrap();
    // Settled, then finished: every step of a deletion reads back as written.
    let settled = read.after_provider_erasure(ProviderSessionErasure::NotListed);
    store.record_deletion(&id, settled.clone()).await.unwrap();

    drop(store);
    let store = LocalConversationStore::open(&path)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    let loaded = ConversationRepository::load(&store, &id)
        .await
        .unwrap()
        .unwrap();
    let tombstone = loaded.deletion().unwrap();
    assert_eq!(tombstone, &settled);
    assert_eq!(
        tombstone.provider_session(),
        &ProviderSessionLink::Recorded(session)
    );
    assert_eq!(
        store.unfinished_deletions().await.unwrap().conversations,
        std::slice::from_ref(&id)
    );
    let finished = settled.after_erasure().unwrap();
    store.record_deletion(&id, finished.clone()).await.unwrap();
    assert_eq!(
        ConversationRepository::load(&store, &id)
            .await
            .unwrap()
            .unwrap()
            .deletion(),
        Some(&finished)
    );
    assert!(store
        .unfinished_deletions()
        .await
        .unwrap()
        .conversations
        .is_empty());
    // Creating the identity again finds it, deleted, rather than making it.
    let created = store.create(owned(&id)).await.unwrap();
    assert_eq!(
        created.disposition,
        ConversationCreationDisposition::Existing
    );
    assert!(created.conversation.deletion().is_some());
}

#[tokio::test]
async fn nothing_stands_without_its_record() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let id = new_id();
    // A summary for a conversation nobody created is refused, not kept.
    assert!(matches!(
        ConversationSummaries::record(&store, &id, said("hello", 5)).await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(
        ConversationSummaries::load(&store, &id).await.unwrap(),
        None
    );
    // And a record with a tombstone or summary beside it cannot be removed
    // from under them.
    store.create(owned(&id)).await.unwrap();
    ConversationSummaries::record(&store, &id, said("hello", 5))
        .await
        .unwrap();
    store
        .record_deletion(&id, deletion("delete"))
        .await
        .unwrap();
    let raw = raw(&path);
    raw.pragma_update(None, "foreign_keys", true).unwrap();
    assert!(raw
        .execute("DELETE FROM conversations WHERE id = ?1", [id.to_string()])
        .is_err());
}

#[tokio::test]
async fn a_row_that_cannot_be_read_is_refused_and_never_read_as_absent() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    // A tombstone that could not stand beside its record is not written.
    let mallory = ConversationDeletion::new(
        org(),
        PrincipalId::new("mallory").unwrap(),
        "panel".into(),
        "delete".into(),
        50,
    )
    .unwrap();
    assert!(matches!(
        store.record_deletion(&id, mallory).await,
        Err(ConversationError::Metadata)
    ));
    assert!(ConversationRepository::load(&store, &id)
        .await
        .unwrap()
        .unwrap()
        .deletion()
        .is_none());
    store
        .record_deletion(&id, deletion("delete"))
        .await
        .unwrap();
    let raw = raw(&path);
    // This test deliberately writes impossible rows to exercise restoration.
    raw.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    // Each change is committed, so the store's own connection reads it, and
    // then put back as the store wrote it.
    let change_deletion = |change: &str| {
        raw.execute(
            &format!("UPDATE deletions SET {change} WHERE conversation_id = ?1"),
            [id.to_string()],
        )
        .unwrap();
    };
    let as_written = "organization = 'org', initiator = 'alice', surface = 'panel', \
                      request = 'delete', requested_at_ms = 50, provider_session = 'unread', \
                      provider_session_id = NULL, provider_erasure = NULL, erased = 0";
    for change in [
        // A provider session that is not one, or read and not read at once.
        "provider_session = 'recorded', provider_session_id = ''",
        "provider_session = 'recorded', provider_session_id = NULL",
        "provider_session_id = 'x'",
        "provider_session = 'misread'",
        // A request that could not have been written down.
        "request = 'line' || char(10) || 'break'",
        // Deleted by somebody who is not the owner, or not in its
        // organization, or before the conversation existed.
        "initiator = 'mallory'",
        "organization = 'elsewhere'",
        "requested_at_ms = 0",
        "requested_at_ms = -1",
        // Progress no deletion reaches: the agent's record settled before the
        // history was read, erased before it was settled, or an erasure this
        // build has no name for.
        "provider_erasure = 'deleted'",
        "provider_session = 'recorded', provider_session_id = 's', erased = 1",
        "provider_session = 'absent'",
        // Settled as naming none, and naming one all the same.
        "provider_session = 'absent', provider_session_id = 'x', \
         provider_erasure = 'no_provider_session'",
        "provider_session = 'recorded', provider_session_id = 's', provider_erasure = 'shredded'",
    ] {
        change_deletion(change);
        assert!(
            matches!(
                ConversationRepository::load(&store, &id).await,
                Err(ConversationError::Metadata)
            ),
            "{change}"
        );
        change_deletion(as_written);
        assert!(ConversationRepository::load(&store, &id).await.is_ok());
    }
    // The same for the record itself: never read as absent, and so never
    // created again.
    for change in [
        "creator_surface = ''",
        "owner = ' alice'",
        "creation_requested_at_ms = -1",
    ] {
        raw.execute(
            &format!("UPDATE conversations SET {change} WHERE id = ?1"),
            [id.to_string()],
        )
        .unwrap();
        assert!(
            matches!(
                ConversationRepository::load(&store, &id).await,
                Err(ConversationError::Metadata)
            ),
            "{change}"
        );
        assert!(
            matches!(
                store.create(owned(&id)).await,
                Err(ConversationError::Metadata)
            ),
            "{change}"
        );
        raw.execute(
            "UPDATE conversations SET creator_surface = 'panel', owner = 'alice', \
             creation_requested_at_ms = 1 WHERE id = ?1",
            [id.to_string()],
        )
        .unwrap();
    }
    assert!(ConversationRepository::load(&store, &id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn a_summary_is_replaced_whole_archived_and_erased() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    let first = said("first words", 5);
    ConversationSummaries::record(&store, &id, first.clone())
        .await
        .unwrap();
    let archived = ConversationSummary::after_reply(Some(&first), "a reply", 9)
        .unwrap()
        .after_archiving(true);
    ConversationSummaries::record(&store, &id, archived.clone())
        .await
        .unwrap();
    drop(store);
    let store = LocalConversationStore::open(&path)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    assert_eq!(
        ConversationSummaries::load(&store, &id).await.unwrap(),
        Some(archived)
    );
    ConversationSummaries::erase(&store, &id).await.unwrap();
    // Erasing what is gone succeeds.
    ConversationSummaries::erase(&store, &id).await.unwrap();
    assert_eq!(
        ConversationSummaries::load(&store, &id).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn a_summary_that_is_not_what_the_rules_produce_is_refused() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    ConversationSummaries::record(&store, &id, said("hello", 5))
        .await
        .unwrap();
    let raw = raw(&path);
    for change in [
        format!("title = '{}'", "x".repeat(200)),
        "title = ''".to_owned(),
        format!("preview = '{}'", "x".repeat(4096)),
        // Past the latest time a list can carry, or before any.
        "updated_at_ms = 9007199254740992".to_owned(),
        "updated_at_ms = -1".to_owned(),
    ] {
        raw.execute(
            &format!("UPDATE summaries SET {change} WHERE conversation_id = ?1"),
            [id.to_string()],
        )
        .unwrap();
        assert!(
            matches!(
                ConversationSummaries::load(&store, &id).await,
                Err(ConversationError::Metadata)
            ),
            "{change}"
        );
        ConversationSummaries::record(&store, &id, said("hello", 5))
            .await
            .unwrap();
    }
    // A flag is 0 or 1, and the file refuses anything else outright.
    assert!(raw
        .execute(
            "UPDATE summaries SET archived = 2 WHERE conversation_id = ?1",
            [id.to_string()],
        )
        .is_err());
}

#[tokio::test]
async fn the_list_is_the_owners_undeleted_said_in_conversations_newest_first() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let (older, newer, tied_low, tied_high) = {
        let mut tied = [new_id(), new_id()];
        tied.sort_by_key(ToString::to_string);
        let [low, high] = tied;
        (new_id(), new_id(), low, high)
    };
    for (id, at) in [
        (&older, 10),
        (&newer, 30),
        (&tied_low, 20),
        (&tied_high, 20),
    ] {
        store.create(owned(&id.clone())).await.unwrap();
        ConversationSummaries::record(&store, id, said("hello", at))
            .await
            .unwrap();
    }
    // Nothing said: no summary, not listed.
    let silent = new_id();
    store.create(owned(&silent)).await.unwrap();
    // Archived: only in the archived list.
    let archived = new_id();
    store.create(owned(&archived)).await.unwrap();
    ConversationSummaries::record(&store, &archived, said("hello", 40).after_archiving(true))
        .await
        .unwrap();
    // Deleted, with its summary not yet erased: in neither.
    let deleted = new_id();
    store.create(owned(&deleted)).await.unwrap();
    ConversationSummaries::record(&store, &deleted, said("hello", 50))
        .await
        .unwrap();
    store
        .record_deletion(&deleted, deletion("delete"))
        .await
        .unwrap();
    // Somebody else's — another organization, and an owner who differs only
    // in case — and one of theirs damaged: none of it is read.
    for (organization, owner) in [("elsewhere", "alice"), ("org", "Alice")] {
        let theirs = new_id();
        store
            .create(owned_by(&theirs, organization, owner, 1))
            .await
            .unwrap();
        ConversationSummaries::record(&store, &theirs, said("hello", 60))
            .await
            .unwrap();
        raw(&path)
            .execute(
                "UPDATE conversations SET creator_surface = '' WHERE id = ?1",
                [theirs.to_string()],
            )
            .unwrap();
    }
    let ordered = vec![
        newer.clone(),
        tied_low.clone(),
        tied_high.clone(),
        older.clone(),
    ];
    assert_eq!(
        listed(&store, &alice(), false, 10).await,
        (ordered.clone(), 0)
    );
    assert_eq!(
        listed(&store, &alice(), true, 10).await,
        (vec![archived.clone()], 0)
    );
    // The limit keeps the newest.
    assert_eq!(
        listed(&store, &alice(), false, 2).await,
        (ordered[..2].to_vec(), 0)
    );
    // A row of the owner's that cannot be read is counted, in the list its
    // flag files it under, and nowhere else.
    raw(&path)
        .execute(
            "UPDATE summaries SET title = '' WHERE conversation_id = ?1",
            [older.to_string()],
        )
        .unwrap();
    assert_eq!(
        listed(&store, &alice(), false, 10).await,
        (ordered[..3].to_vec(), 1)
    );
    assert_eq!(
        listed(&store, &alice(), true, 10).await,
        (vec![archived.clone()], 0)
    );
    // A row naming its conversation other than the one way the server writes
    // it is not read as that conversation: counted, not listed. Renamed on
    // both sides at once, which only a hand edit with the keys off could do.
    let raw = raw(&path);
    raw.pragma_update(None, "foreign_keys", false).unwrap();
    for statement in [
        "UPDATE summaries SET conversation_id = upper(conversation_id) WHERE conversation_id = ?1",
        "UPDATE conversations SET id = upper(id) WHERE id = ?1",
    ] {
        raw.execute(statement, [newer.to_string()]).unwrap();
    }
    assert_eq!(
        listed(&store, &alice(), false, 10).await,
        (ordered[1..3].to_vec(), 2)
    );
}

#[tokio::test]
async fn the_list_asks_whose_a_conversation_is_as_the_domain_answers_it() {
    let Opened {
        _directory, store, ..
    } = opened();
    // Owner text chosen to separate an exact comparison from any looser one:
    // case, composed and decomposed accents, an invisible character, a
    // lookalike letter from another script, and another organization.
    let organizations = ["org", "Org"];
    let owners = [
        "alice",
        "Alice",
        "ALICE",
        "\u{e9}",
        "e\u{301}",
        "a\u{200b}lice",
        "\u{430}lice",
    ];
    let mut all = Vec::new();
    for organization in organizations {
        for owner in owners {
            let id = new_id();
            store
                .create(owned_by(&id, organization, owner, 1))
                .await
                .unwrap();
            ConversationSummaries::record(&store, &id, said("hello", 5))
                .await
                .unwrap();
            all.push(
                ConversationRepository::load(&store, &id)
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
    }
    for organization in organizations {
        let organization = OrganizationId::new(organization).unwrap();
        for owner in owners {
            let owner = PrincipalId::new(owner).unwrap();
            let listed = ConversationListing::list(&store, &organization, &owner, false, 100)
                .await
                .unwrap();
            let mut listed: Vec<String> = listed
                .conversations
                .iter()
                .map(|listed| listed.conversation.id().to_string())
                .collect();
            let mut allowed: Vec<String> = all
                .iter()
                .filter(|conversation| conversation.allows(&organization, &owner))
                .map(|conversation| conversation.id().to_string())
                .collect();
            listed.sort();
            allowed.sort();
            assert_eq!(listed, allowed, "{organization:?} {owner:?}");
            assert_eq!(listed.len(), 1, "{organization:?} {owner:?}");
        }
    }
}

#[tokio::test]
async fn a_tombstone_that_cannot_be_read_still_keeps_its_conversation_out_of_every_list() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    ConversationSummaries::record(&store, &id, said("hello", 5))
        .await
        .unwrap();
    store
        .record_deletion(&id, deletion("delete"))
        .await
        .unwrap();
    raw(&path)
        .execute(
            "UPDATE deletions SET initiator = 'mallory' WHERE conversation_id = ?1",
            [id.to_string()],
        )
        .unwrap();
    // Deleted whatever its tombstone says: in neither list, and neither list
    // is short of anything the caller could be shown.
    for archived in [false, true] {
        assert_eq!(listed(&store, &alice(), archived, 10).await, (vec![], 0));
    }
}

#[tokio::test]
async fn a_row_whose_text_is_not_utf8_costs_its_list_that_row_alone() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let (kept, damaged) = (new_id(), new_id());
    for (id, at) in [(&kept, 5), (&damaged, 6)] {
        store.create(owned(id)).await.unwrap();
        ConversationSummaries::record(&store, id, said("hello", at))
            .await
            .unwrap();
    }
    // `STRICT` takes any bytes as text; only reading them back finds out.
    let raw = raw(&path);
    raw.execute(
        "UPDATE summaries SET title = CAST(x'ff' AS TEXT) WHERE conversation_id = ?1",
        [damaged.to_string()],
    )
    .unwrap();
    assert_eq!(
        listed(&store, &alice(), false, 10).await,
        (vec![kept.clone()], 1)
    );
    // And a tombstone whose conversation is named in such bytes costs the
    // startup finish that deletion alone.
    let unfinished = new_id();
    store.create(owned(&unfinished)).await.unwrap();
    store
        .record_deletion(&unfinished, deletion("delete"))
        .await
        .unwrap();
    raw.pragma_update(None, "foreign_keys", false).unwrap();
    raw.execute(
        "INSERT INTO deletions VALUES (CAST(x'ff' AS TEXT), 'org', 'alice', 'panel', 'delete', 50, 'unread', NULL, NULL, 0)",
        [],
    )
    .unwrap();
    let found = store.unfinished_deletions().await.unwrap();
    assert_eq!(
        (found.conversations, found.unreadable),
        (vec![unfinished], 1)
    );
}

#[tokio::test]
async fn oversized_text_costs_its_list_one_row_and_remains_refused_by_exact_reads() {
    for (table, column, ceiling) in [
        ("conversations", "model", ConversationModelId::MAX_BYTES),
        (
            "conversations",
            "creator_surface",
            Conversation::MAX_CREATOR_CONTEXT_BYTES,
        ),
        (
            "conversations",
            "creation_action",
            Conversation::MAX_CREATOR_CONTEXT_BYTES,
        ),
        (
            "summaries",
            "title",
            ConversationTitle::MAX_CHARS * char::MAX.len_utf8(),
        ),
        ("summaries", "preview", ConversationPreview::MAX_BYTES),
    ] {
        let opened = opened();
        let (kept, damaged) = (new_id(), new_id());
        for (id, at) in [(&kept, 5), (&damaged, 6)] {
            opened.store.create(owned(id)).await.unwrap();
            ConversationSummaries::record(&opened.store, id, said("hello", at))
                .await
                .unwrap();
        }
        let key = if table == "conversations" {
            "id"
        } else {
            "conversation_id"
        };
        raw(&opened.path)
            .execute(
                &format!("UPDATE {table} SET {column} = ?1 WHERE {key} = ?2"),
                params!["x".repeat(ceiling * 3), damaged.to_string()],
            )
            .unwrap();
        assert_eq!(
            listed(&opened.store, &alice(), false, 10).await,
            (vec![kept.clone()], 1),
            "{column}"
        );
        assert_eq!(
            listed(&opened.store, &PrincipalId::new("bob").unwrap(), false, 10).await,
            (vec![], 0)
        );
        let exact = if table == "conversations" {
            ConversationRepository::load(&opened.store, &damaged)
                .await
                .map(|_| ())
        } else {
            ConversationSummaries::load(&opened.store, &damaged)
                .await
                .map(|_| ())
        };
        assert!(
            matches!(exact, Err(ConversationError::Metadata)),
            "{column}"
        );
        let head = opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner,
            )
            .await
            .unwrap();
        assert!(
            matches!(
                opened
                    .store
                    .resolve(
                        &org(),
                        &alice(),
                        &Reader::Owner,
                        &head.incarnation,
                        &damaged
                    )
                    .await,
                Err(ConversationError::Metadata)
            ),
            "{column}"
        );
        let reopened = LocalConversationStore::open(&opened.path)
            .unwrap()
            .with_access_policy(crate::conversation_test_support::access_policy());
        assert_eq!(
            listed(&reopened, &alice(), false, 10).await,
            (vec![kept], 1),
            "{column} after reopen"
        );
    }
}

#[tokio::test]
async fn a_failed_list_query_is_not_reported_as_row_damage() {
    let opened = opened();
    raw(&opened.path)
        .execute("DROP TABLE summaries", [])
        .unwrap();
    assert!(matches!(
        ConversationListing::list(&opened.store, &org(), &alice(), false, 10).await,
        Err(ConversationError::Metadata)
    ));
}

#[test]
fn a_file_that_is_not_a_database_is_refused_as_unreadable() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    // Private, so it is SQLite, not the privacy check, that refuses it.
    let mut file =
        nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew).unwrap();
    std::io::Write::write_all(&mut file, &[7; 4096]).unwrap();
    drop(file);
    assert!(matches!(
        LocalConversationStore::open(&path),
        Err(nessa_local_database::OpenError::Unreadable(_))
    ));
}

/// Rows of `count` conversations owned by `owner`, each with a summary,
/// written in one transaction.
fn many(path: &Path, owner: &str, count: usize) {
    let mut raw = raw(path);
    let transaction = raw.transaction().unwrap();
    transaction.execute(
        "INSERT OR IGNORE INTO catalogue_owners (organization, owner, head) VALUES ('org', ?1, 0)",
        [owner],
    ).unwrap();
    for n in 0..count {
        let id = new_id().to_string();
        let revision: i64 = transaction.query_row(
            "UPDATE catalogue_owners SET head = head + 1 WHERE organization = 'org' AND owner = ?1 RETURNING head",
            [owner], |row| row.get(0),
        ).unwrap();
        transaction
            .execute(
                "INSERT INTO conversations VALUES (?1, 'org', ?2, 'panel', 'create', 1, 'claude', 'test-model', 'ask', ?3, ?3)",
                params![id, owner, revision],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO summaries VALUES (?1, 'Hello', 'hello', ?2, 0)",
                params![id, 10 + n as i64],
            )
            .unwrap();
    }
    transaction.commit().unwrap();
}

/// How many virtual-machine steps, and full-scan steps, `LIST` takes for
/// `owner`, run as the store runs it.
fn list_cost(path: &Path, owner: &str, limit: i64) -> (usize, i32, i32) {
    let raw = raw(path);
    let mut statement = raw.prepare(&list_query()).unwrap();
    let rows = statement
        .query_map(params!["org", owner, false, limit], |_| Ok(()))
        .unwrap()
        .count();
    (
        rows,
        statement.get_status(StatementStatus::VmStep),
        statement.get_status(StatementStatus::FullscanStep),
    )
}

#[tokio::test]
async fn the_list_query_reads_only_its_owners_rows_however_many_others_there_are() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    many(&path, "alice", 3);
    many(&path, "someone", 1_000);
    let (rows, few_others, scanned) = list_cost(&path, "alice", 501);
    assert_eq!((rows, scanned), (3, 0));
    // Fifty times as many conversations that are somebody else's cost the
    // owner's list nothing more.
    many(&path, "someone", 49_000);
    let (rows, many_others, scanned) = list_cost(&path, "alice", 501);
    assert_eq!((rows, scanned), (3, 0));
    assert_eq!(few_others, many_others);
    // And the store's own list of them, through the port, is theirs alone.
    let (ids, unreadable) = listed(&store, &alice(), false, 501).await;
    assert_eq!((ids.len(), unreadable), (3, 0));
    // An owner with more than the bound gets the bound and one more.
    let (rows, _, scanned) = list_cost(&path, "someone", 501);
    assert_eq!((rows, scanned), (501, 0));
}

#[tokio::test]
async fn unfinished_deletions_are_read_by_their_index_and_an_unnamed_one_is_counted() {
    let Opened {
        _directory,
        path,
        store,
    } = opened();
    let (unfinished, finished, alive) = (new_id(), new_id(), new_id());
    for id in [&unfinished, &finished, &alive] {
        store.create(owned(id)).await.unwrap();
    }
    store
        .record_deletion(&unfinished, deletion("delete"))
        .await
        .unwrap();
    store
        .record_deletion(
            &finished,
            deletion("delete")
                .after_reading(None)
                .after_erasure()
                .unwrap(),
        )
        .await
        .unwrap();
    let found = store.unfinished_deletions().await.unwrap();
    assert_eq!(
        (found.conversations, found.unreadable),
        (vec![unfinished.clone()], 0)
    );
    let plan: Vec<String> = raw(&path)
        .prepare(&format!("EXPLAIN QUERY PLAN {UNFINISHED}"))
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        plan.iter()
            .any(|step| step.contains("unfinished_deletions")),
        "{plan:?}"
    );
    // One whose conversation cannot even be named: counted, not guessed at.
    let unnamed = "not-a-conversation";
    let raw = raw(&path);
    raw.execute(
        "INSERT INTO conversations VALUES (?1, 'org', 'alice', 'panel', 'create', 1, 'claude', 'test-model', 'ask', 1, 1)",
        [unnamed],
    )
    .unwrap();
    raw.execute(
        "INSERT INTO deletions VALUES (?1, 'org', 'alice', 'panel', 'delete', 50, 'unread', NULL, NULL, 0)",
        [unnamed],
    )
    .unwrap();
    let found = store.unfinished_deletions().await.unwrap();
    assert_eq!(
        (found.conversations, found.unreadable),
        (vec![unfinished], 1)
    );
}

#[tokio::test]
async fn catalogue_revisions_follow_owner_visible_changes_and_survive_restart() {
    let opened = opened();
    let id = new_id();
    let bob = PrincipalId::new("bob").unwrap();
    let incarnation = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .incarnation;
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        0
    );
    opened.store.create(owned(&id)).await.unwrap();
    let first = opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (first.descriptor.key.creation, first.descriptor.revision),
        (1, 1)
    );
    assert!(first.summary.is_none());
    assert!(opened
        .store
        .resolve(&org(), &bob, &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .is_none());
    opened.store.record(&id, said("hello", 2)).await.unwrap();
    let second = opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (second.descriptor.key.creation, second.descriptor.revision),
        (1, 2)
    );
    assert!(second.summary.is_some());
    opened
        .store
        .record(&id, second.summary.unwrap().after_archiving(true))
        .await
        .unwrap();
    let archived = opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(archived.descriptor.revision, 3);
    assert!(archived.summary.unwrap().archived());
    opened
        .store
        .record_deletion(&id, deletion("delete"))
        .await
        .unwrap();
    let deleted = opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (deleted.descriptor.key.creation, deleted.descriptor.revision),
        (1, 4)
    );
    assert!(deleted.descriptor.deleted);
    assert!(deleted.summary.is_none());
    assert!(matches!(
        opened.store.record(&id, said("late", 3)).await,
        Err(ConversationError::Deleted)
    ));
    opened
        .store
        .record_deletion(&id, deletion("again"))
        .await
        .unwrap();
    opened.store.erase(&id).await.unwrap();
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        4
    );
    drop(opened.store);
    let reopened = LocalConversationStore::open(&opened.path)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    assert_eq!(
        reopened
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .incarnation,
        incarnation
    );
    assert!(
        reopened
            .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
            .await
            .unwrap()
            .unwrap()
            .descriptor
            .deleted
    );
    assert!(matches!(
        reopened
            .page(catalogue_request(alice(), "wrong", 0, 3, None, 2))
            .await,
        Err(ConversationError::CatalogueIdentityChanged)
    ));
}

#[tokio::test]
async fn catalogue_pages_use_creation_keys_and_do_not_cut_off_newer_changes() {
    let opened = opened();
    let incarnation = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .incarnation;
    let ids: Vec<_> = (0..620).map(|_| new_id()).collect();
    for id in &ids {
        opened.store.create(owned(id)).await.unwrap();
    }
    for _ in 0..20 {
        let id = new_id();
        opened
            .store
            .create(owned_by(&id, "org", "bob", 1))
            .await
            .unwrap();
    }
    let bob = PrincipalId::new("bob").unwrap();
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &bob,
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        20
    );
    assert_eq!(
        opened
            .store
            .page(catalogue_request(bob, &incarnation, 0, 20, None, 21))
            .await
            .unwrap()
            .entries
            .len(),
        20
    );
    let boundary = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .revision;
    let writer = LocalConversationStore::open(&opened.path)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    let created_midpass = new_id();
    writer.create(owned(&created_midpass)).await.unwrap();
    let mut after = None;
    let mut seen = Vec::new();
    let mut first_page = true;
    loop {
        let page = opened
            .store
            .page(catalogue_request(
                alice(),
                &incarnation,
                0,
                boundary,
                after.clone(),
                37,
            ))
            .await
            .unwrap();
        if let Some(last) = page.entries.last() {
            after = Some(last.key.clone());
        }
        seen.extend(page.entries);
        if first_page {
            writer
                .record(&ids[0], said("changed behind cursor", 3))
                .await
                .unwrap();
            first_page = false;
        }
        if !page.has_more {
            break;
        }
    }
    assert_eq!(seen.len(), 620);
    assert!(seen
        .windows(2)
        .all(|pair| (pair[0].key.creation, pair[0].key.id.to_string())
            < (pair[1].key.creation, pair[1].key.id.to_string())));
    let late = ids.last().unwrap();
    writer.record(late, said("late", 4)).await.unwrap();
    let page = opened
        .store
        .page(catalogue_request(
            alice(),
            &incarnation,
            0,
            boundary,
            Some(seen[618].key.clone()),
            2,
        ))
        .await
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].revision, boundary + 3);
    assert_eq!(page.entries[0].key.creation, boundary);
    let next = opened
        .store
        .page(catalogue_request(
            alice(),
            &incarnation,
            boundary,
            boundary + 3,
            None,
            3,
        ))
        .await
        .unwrap();
    assert_eq!(next.entries.len(), 3);
    assert_eq!(next.entries[0].key.id, ids[0]);
    assert_eq!(next.entries[1].key.id, *late);
    assert_eq!(next.entries[2].key.id, created_midpass);
}

#[tokio::test]
async fn catalogue_refuses_its_owners_damaged_row_without_reading_another_owners() {
    let opened = opened();
    let ours = new_id();
    let theirs = new_id();
    opened.store.create(owned(&ours)).await.unwrap();
    opened
        .store
        .create(owned_by(&theirs, "org", "bob", 1))
        .await
        .unwrap();
    let incarnation = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .incarnation;
    raw(&opened.path)
        .execute(
            "UPDATE conversations SET creator_surface = CAST(x'80' AS TEXT) WHERE id = ?1",
            [theirs.to_string()],
        )
        .unwrap();
    let ours_page = opened
        .store
        .page(catalogue_request(alice(), &incarnation, 0, 1, None, 2))
        .await
        .unwrap();
    assert_eq!(ours_page.entries.len(), 1);
    assert_eq!(ours_page.entries[0].key.id, ours);
    let bob = PrincipalId::new("bob").unwrap();
    assert!(matches!(
        opened
            .store
            .page(catalogue_request(bob, &incarnation, 0, 1, None, 2))
            .await,
        Err(ConversationError::Metadata)
    ));
}

#[tokio::test]
async fn catalogue_resolves_a_deletion_that_raced_its_manifest_descriptor() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let incarnation = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .incarnation;
    let page = opened
        .store
        .page(catalogue_request(alice(), &incarnation, 0, 1, None, 1))
        .await
        .unwrap();
    assert!(!page.entries[0].deleted);
    assert_eq!(page.entries[0].revision, 1);
    opened
        .store
        .record_deletion(&id, deletion("raced"))
        .await
        .unwrap();
    let current = opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.descriptor.key, page.entries[0].key);
    assert_eq!(current.descriptor.revision, 2);
    assert!(current.descriptor.deleted);
    assert!(matches!(
        opened
            .store
            .page(catalogue_request(alice(), &incarnation, 0, 3, None, 1))
            .await,
        Err(ConversationError::CatalogueInvalidRequest)
    ));
    assert!(matches!(
        opened
            .store
            .page(catalogue_request(alice(), &incarnation, 0, 2, None, 0))
            .await,
        Err(ConversationError::CatalogueInvalidRequest)
    ));
    let other = PrincipalId::new("bob").unwrap();
    assert!(opened
        .store
        .resolve(&org(), &other, &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_failed_visible_write_rolls_back_its_owner_revision() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let incarnation = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .incarnation;
    let raw = raw(&opened.path);
    raw.execute_batch("CREATE TRIGGER refuse_summary BEFORE INSERT ON summaries BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
    assert!(matches!(
        opened.store.record(&id, said("refused", 2)).await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        1
    );
    assert!(opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .unwrap()
        .summary
        .is_none());
    raw.execute_batch("DROP TRIGGER refuse_summary;").unwrap();
    opened.store.record(&id, said("accepted", 3)).await.unwrap();
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        2
    );
    raw.execute_batch("CREATE TRIGGER refuse_deletion BEFORE INSERT ON deletions BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
    assert!(matches!(
        opened.store.record_deletion(&id, deletion("refused")).await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        2
    );
    assert!(
        !opened
            .store
            .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
            .await
            .unwrap()
            .unwrap()
            .descriptor
            .deleted
    );
    raw.execute_batch("DROP TRIGGER refuse_deletion;").unwrap();
    opened
        .store
        .record_deletion(&id, deletion("accepted"))
        .await
        .unwrap();
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        3
    );
}

#[tokio::test]
async fn a_failed_creation_does_not_publish_an_owner_or_spend_a_revision() {
    let opened = opened();
    let id = new_id();
    let incarnation = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .incarnation;
    let raw = raw(&opened.path);
    raw.execute_batch("CREATE TRIGGER refuse_creation BEFORE INSERT ON conversations BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
    assert!(matches!(
        opened.store.create(owned(&id)).await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        0
    );
    assert!(opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .is_none());
    raw.execute_batch("DROP TRIGGER refuse_creation;").unwrap();
    opened.store.create(owned(&id)).await.unwrap();
    let current = opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (current.descriptor.key.creation, current.descriptor.revision),
        (1, 1)
    );
}

#[tokio::test]
async fn a_missing_owner_head_cannot_publish_empty_or_reuse_a_revision() {
    let opened = opened();
    let live = new_id();
    let deleted = new_id();
    let foreign = new_id();
    opened.store.create(owned(&live)).await.unwrap();
    opened.store.create(owned(&deleted)).await.unwrap();
    opened
        .store
        .record_deletion(&deleted, deletion("delete"))
        .await
        .unwrap();
    opened
        .store
        .create(owned_by(&foreign, "org", "bob", 1))
        .await
        .unwrap();
    let incarnation = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .incarnation;
    let raw = raw(&opened.path);
    raw.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    raw.execute(
        "DELETE FROM catalogue_owners WHERE organization = 'org' AND owner = 'alice'",
        [],
    )
    .unwrap();
    assert!(matches!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await,
        Err(ConversationError::Metadata)
    ));
    assert!(matches!(
        opened
            .store
            .page(catalogue_request(alice(), &incarnation, 0, 3, None, 4))
            .await,
        Err(ConversationError::Metadata)
    ));
    assert!(matches!(
        opened
            .store
            .resolve(&org(), &alice(), &Reader::Owner, &incarnation, &live)
            .await,
        Err(ConversationError::Metadata)
    ));
    let newcomer = new_id();
    assert!(matches!(
        opened.store.create(owned(&newcomer)).await,
        Err(ConversationError::Metadata)
    ));
    assert!(ConversationRepository::load(&opened.store, &newcomer)
        .await
        .unwrap()
        .is_none());
    assert!(ConversationRepository::load(&opened.store, &live)
        .await
        .unwrap()
        .is_some());
    assert!(ConversationRepository::load(&opened.store, &deleted)
        .await
        .unwrap()
        .unwrap()
        .deletion()
        .is_some());
    let bob = PrincipalId::new("bob").unwrap();
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &bob,
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        1
    );
    assert!(opened
        .store
        .resolve(&org(), &bob, &Reader::Owner, &incarnation, &foreign)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn a_regressed_owner_head_is_unavailable_before_a_new_revision() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    opened.store.record(&id, said("hello", 2)).await.unwrap();
    let raw = raw(&opened.path);
    raw.execute(
        "UPDATE catalogue_owners SET head = 1 WHERE organization = 'org' AND owner = 'alice'",
        [],
    )
    .unwrap();
    assert!(matches!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await,
        Err(ConversationError::Metadata)
    ));
    assert!(matches!(
        opened.store.record(&id, said("later", 3)).await,
        Err(ConversationError::Metadata)
    ));
    let revision: i64 = raw
        .query_row(
            "SELECT change_revision FROM conversations WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revision, 2);
}

#[tokio::test]
async fn catalogue_metadata_consumes_actual_request_owner_and_derives_one_incarnation() {
    let opened = opened();
    let conversation = new_id();
    opened.store.create(owned(&conversation)).await.unwrap();
    let head = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap();
    let valid = catalogue_request(alice(), &head.incarnation, 0, head.revision, None, 1);
    assert_eq!(
        opened
            .store
            .page(valid.clone())
            .await
            .unwrap()
            .entries
            .len(),
        1
    );
    let mut cases = vec![];
    let mut request = valid.clone();
    request.manifest.pass.generation = 0;
    cases.push(request);
    let mut request = valid.clone();
    request.manifest.pass.boundary = 0;
    cases.push(request);
    for creation in [0, head.revision + 1] {
        let mut request = valid.clone();
        request.manifest.pass.cursor = Some(EntryKey {
            creation,
            id: Id::new(conversation.to_string()).unwrap(),
        });
        cases.push(request);
    }
    for count in [0, MAX_CATALOGUE_ENTRIES + 1] {
        let mut request = valid.clone();
        request.manifest.max_entries = count;
        cases.push(request);
    }
    for request in cases {
        assert!(matches!(
            opened.store.page(request).await,
            Err(ConversationError::CatalogueInvalidRequest)
        ));
    }
    let mut changed = valid.clone();
    let scope = &changed.manifest.pass.scope;
    changed.manifest.pass.scope = Scope::new(
        scope.receiver().clone(),
        scope.origin().clone(),
        scope.stream().clone(),
        Id::new("otherwise-valid-other-incarnation").unwrap(),
        scope.schema().clone(),
        scope.access_epoch().clone(),
    );
    assert!(matches!(
        opened.store.page(changed).await,
        Err(ConversationError::CatalogueIdentityChanged)
    ));
    assert_eq!(opened.store.page(valid).await.unwrap().entries.len(), 1);
}

#[tokio::test]
async fn catalogue_acquisition_refuses_corruption_atomically_and_preserves_unknown_agent() {
    let opened = opened();
    let first = new_id();
    let second = new_id();
    opened.store.create(owned(&first)).await.unwrap();
    opened.store.create(owned(&second)).await.unwrap();
    let head = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap();
    let request = catalogue_request(alice(), &head.incarnation, 0, head.revision, None, 2);
    let connection = raw(&opened.path);
    connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    let huge = "unknown".repeat(1_000_000);
    connection
        .execute(
            "UPDATE conversations SET agent = ?1 WHERE id = ?2",
            params![huge, second.to_string()],
        )
        .unwrap();
    assert_eq!(
        opened
            .store
            .page(request.clone())
            .await
            .unwrap()
            .entries
            .len(),
        2
    );
    let value = opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &head.incarnation, &second)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(value.conversation.agent(), None);
    for column in [
        "model",
        "creator_surface",
        "creation_action",
        "approval_mode",
    ] {
        let original: String = connection
            .query_row(
                &format!("SELECT {column} FROM conversations WHERE id = ?1"),
                [second.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute(
                &format!("UPDATE conversations SET {column} = ?1 WHERE id = ?2"),
                params!["x".repeat(1_000_000), second.to_string()],
            )
            .unwrap();
        // First row remains valid: a later corrupt row must refuse the whole page.
        assert!(opened.store.page(request.clone()).await.is_err());
        assert!(opened
            .store
            .resolve(&org(), &alice(), &Reader::Owner, &head.incarnation, &second)
            .await
            .is_err());
        let retained: i64 = connection
            .query_row(
                &format!("SELECT octet_length({column}) FROM conversations WHERE id = ?1"),
                [second.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 1_000_000);
        connection
            .execute(
                &format!("UPDATE conversations SET {column} = ?1 WHERE id = ?2"),
                params![original, second.to_string()],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO summaries VALUES (?1, ?2, ?3, 1, 0)",
            params![
                second.to_string(),
                "😀".repeat(ConversationTitle::MAX_CHARS),
                "é".repeat(ConversationPreview::MAX_BYTES / 2)
            ],
        )
        .unwrap();
    assert!(opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &head.incarnation, &second)
        .await
        .unwrap()
        .unwrap()
        .summary
        .is_some());
    connection
        .execute(
            "UPDATE summaries SET preview = ?1 WHERE conversation_id = ?2",
            params!["x".repeat(1_000_000), second.to_string()],
        )
        .unwrap();
    assert!(opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &head.incarnation, &second)
        .await
        .is_err());
    connection
        .execute(
            "UPDATE catalogue_identity SET incarnation = ?1",
            ["x".repeat(1_000_000)],
        )
        .unwrap();
    assert!(opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner
        )
        .await
        .is_err());
    assert!(opened.store.page(request).await.is_err());
    assert!(opened
        .store
        .resolve(&org(), &alice(), &Reader::Owner, &head.incarnation, &first)
        .await
        .is_err());
    assert_eq!(
        connection
            .query_row(
                "SELECT octet_length(incarnation) FROM catalogue_identity",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1_000_000
    );
}

#[tokio::test]
async fn catalogue_acquisition_accepts_full_supported_multibyte_fields() {
    let opened = opened();
    let id = new_id();
    let organization = OrganizationId::new("é".repeat(MAX_IDENTIFIER_BYTES / 2)).unwrap();
    let owner = PrincipalId::new("é".repeat(MAX_IDENTIFIER_BYTES / 2)).unwrap();
    let conversation = Conversation::new(
        id.clone(),
        organization.clone(),
        owner.clone(),
        "é".repeat(Conversation::MAX_CREATOR_CONTEXT_BYTES / 2),
        "é".repeat(Conversation::MAX_CREATOR_CONTEXT_BYTES / 2),
        1,
        AgentId::Opencode,
        ConversationModelId::new("é".repeat(ConversationModelId::MAX_BYTES / 2)).unwrap(),
        ConversationApprovalMode::Full,
    )
    .unwrap();
    opened.store.create(conversation).await.unwrap();
    let head = opened
        .store
        .head(
            &organization,
            &owner,
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap();
    let mut request =
        catalogue_request(owner.clone(), &head.incarnation, 0, head.revision, None, 1);
    request.organization = organization.clone();
    let old_scope = &request.manifest.pass.scope;
    request.manifest.pass.scope = Scope::new(
        old_scope.receiver().clone(),
        old_scope.origin().clone(),
        conversation_catalogue_stream(&organization, &owner),
        old_scope.incarnation().clone(),
        old_scope.schema().clone(),
        old_scope.access_epoch().clone(),
    );
    assert_eq!(opened.store.page(request).await.unwrap().entries.len(), 1);
    let value = opened
        .store
        .resolve(
            &organization,
            &owner,
            &Reader::Owner,
            &head.incarnation,
            &id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(value.conversation.agent(), Some(AgentId::Opencode));
    assert_eq!(
        value.conversation.model().as_str().len(),
        ConversationModelId::MAX_BYTES
    );
    assert!(matches!(
        opened
            .store
            .resolve(
                &organization,
                &owner,
                &Reader::Owner,
                &"x".repeat(1_000_000),
                &id
            )
            .await,
        Err(ConversationError::CatalogueInvalidRequest)
    ));
}

#[tokio::test]
async fn catalogue_resolve_acquires_bounded_deletion_and_preserves_supported_progress() {
    let opened = opened();
    let session = ExecutionSessionId::new("é".repeat(ExecutionSessionId::MAX_BYTES / 2)).unwrap();
    let initial = deletion("delete");
    let recorded = initial.after_reading(Some(session));
    let mut supported = vec![
        initial,
        deletion("delete").after_reading(None),
        deletion("delete").after_losing_history(),
        recorded.clone(),
    ];
    for erasure in [
        ProviderSessionErasure::Deleted,
        ProviderSessionErasure::Archived,
        ProviderSessionErasure::Acknowledged,
        ProviderSessionErasure::NotListed,
        ProviderSessionErasure::NotSupported,
        ProviderSessionErasure::NoHandler,
    ] {
        supported.push(recorded.after_provider_erasure(erasure));
    }
    for decision in supported {
        let id = new_id();
        opened.store.create(owned(&id)).await.unwrap();
        opened
            .store
            .record_deletion(&id, decision.clone())
            .await
            .unwrap();
        let head = opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner,
            )
            .await
            .unwrap();
        let value = opened
            .store
            .resolve(&org(), &alice(), &Reader::Owner, &head.incarnation, &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(value.conversation.deletion(), Some(&decision));
    }
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    opened.store.record_deletion(&id, recorded).await.unwrap();
    let head = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap();
    let connection = raw(&opened.path);
    connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    for column in [
        "organization",
        "initiator",
        "surface",
        "request",
        "provider_session",
        "provider_session_id",
        "provider_erasure",
    ] {
        let prior: Option<String> = connection
            .query_row(
                &format!("SELECT {column} FROM deletions WHERE conversation_id = ?1"),
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute(
                &format!("UPDATE deletions SET {column} = ?1 WHERE conversation_id = ?2"),
                params!["x".repeat(1_000_000), id.to_string()],
            )
            .unwrap();
        assert!(
            opened
                .store
                .resolve(&org(), &alice(), &Reader::Owner, &head.incarnation, &id)
                .await
                .is_err(),
            "{column}"
        );
        assert_eq!(
            connection
                .query_row(
                    &format!(
                        "SELECT octet_length({column}) FROM deletions WHERE conversation_id = ?1"
                    ),
                    [id.to_string()],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1_000_000
        );
        connection
            .execute(
                &format!("UPDATE deletions SET {column} = ?1 WHERE conversation_id = ?2"),
                params![prior, id.to_string()],
            )
            .unwrap();
        assert!(opened
            .store
            .resolve(&org(), &alice(), &Reader::Owner, &head.incarnation, &id)
            .await
            .is_ok());
    }
}

fn catalogue_watch_ready(watch: &mut CatalogueChangeWatch) -> CatalogueWatchState {
    let mut wait = Box::pin(watch.changed());
    match wait.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(state) => state,
        Poll::Pending => panic!("expected a committed source notice"),
    }
}

fn catalogue_watch_pending(watch: &mut CatalogueChangeWatch) {
    let mut wait = Box::pin(watch.changed());
    assert!(matches!(
        wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
}

#[tokio::test]
async fn catalogue_watch_tracks_only_committed_owner_revisions_and_tombstone_fence() {
    let opened = opened();
    let id = new_id();
    let mut watch = opened.store.watch(&org(), &alice()).unwrap();
    let mut other = opened
        .store
        .watch(&org(), &PrincipalId::new("bob").unwrap())
        .unwrap();
    opened.store.create(owned(&id)).await.unwrap();
    assert_eq!(
        catalogue_watch_ready(&mut watch),
        CatalogueWatchState::Dirty
    );
    catalogue_watch_pending(&mut other);
    // Already-created identity does not advance head or notify.
    opened.store.create(owned(&id)).await.unwrap();
    catalogue_watch_pending(&mut watch);
    let mut late = opened.store.watch(&org(), &alice()).unwrap();
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        1
    );
    catalogue_watch_pending(&mut late);
    opened.store.record(&id, said("visible", 2)).await.unwrap();
    assert_eq!(
        catalogue_watch_ready(&mut watch),
        CatalogueWatchState::Dirty
    );
    assert_eq!(catalogue_watch_ready(&mut late), CatalogueWatchState::Dirty);
    opened.store.erase(&id).await.unwrap();
    assert_eq!(
        catalogue_watch_ready(&mut watch),
        CatalogueWatchState::Dirty
    );
    assert_eq!(catalogue_watch_ready(&mut late), CatalogueWatchState::Dirty);
    // First tombstone notifies before any provider/upload/history cleanup.
    opened
        .store
        .record_deletion(&id, deletion("delete-watch"))
        .await
        .unwrap();
    assert_eq!(
        catalogue_watch_ready(&mut watch),
        CatalogueWatchState::Dirty
    );
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        4
    );
    assert!(matches!(
        opened
            .store
            .record(&id, said("refused after fence", 3))
            .await,
        Err(ConversationError::Deleted)
    ));
    opened
        .store
        .record_deletion(&id, deletion("delete-watch"))
        .await
        .unwrap();
    opened.store.erase(&id).await.unwrap();
    catalogue_watch_pending(&mut watch);
    catalogue_watch_pending(&mut other);
    drop(opened);
    assert_eq!(
        catalogue_watch_ready(&mut watch),
        CatalogueWatchState::Closed
    );
}

#[tokio::test]
async fn catalogue_watch_refuses_to_publish_a_rolled_back_revision() {
    let opened = opened();
    let id = new_id();
    let mut watch = opened.store.watch(&org(), &alice()).unwrap();
    {
        let connection = opened.store.hold_mutation();
        // All writes succeed inside the transaction; the deferred foreign key
        // refuses COMMIT. This detects notification moved just before commit.
        connection.execute_batch("CREATE TRIGGER refuse_creation AFTER INSERT ON conversations BEGIN INSERT INTO summaries (conversation_id, updated_at_ms, archived) VALUES ('missing-watch-target', 1, 0); END; PRAGMA defer_foreign_keys = ON;").unwrap();
    }
    assert!(matches!(
        opened.store.create(owned(&id)).await,
        Err(ConversationError::Metadata)
    ));
    catalogue_watch_pending(&mut watch);
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        0
    );
    raw(&opened.path)
        .execute_batch("DROP TRIGGER refuse_creation;")
        .unwrap();
    opened.store.create(owned(&id)).await.unwrap();
    assert_eq!(
        catalogue_watch_ready(&mut watch),
        CatalogueWatchState::Dirty
    );
}

#[tokio::test]
async fn cancelled_catalogue_receipt_keeps_commit_publication_alive() {
    let opened = opened();
    let id = new_id();
    let mut watch = opened.store.watch(&org(), &alice()).unwrap();
    // Hold the actual connection so the blocking mutation cannot complete
    // during its first poll; drop the receipt, then release physical work.
    {
        let gate = opened.store.hold_mutation();
        let mut creation = opened.store.create(owned(&id));
        assert!(matches!(
            creation
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        drop(creation);
        drop(gate);
    }
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), watch.changed())
            .await
            .unwrap(),
        CatalogueWatchState::Dirty
    );
    assert!(ConversationRepository::load(&opened.store, &id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn catalogue_watch_mode_bookkeeping_stays_clean_until_visible_mode_commit() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let mut watch = opened.store.watch(&org(), &alice()).unwrap();
    let request = ConversationModeRequest {
        conversation_id: id.clone(),
        organization_id: org(),
        request_id: "watch-mode".into(),
        initiator_principal_id: alice(),
        initiator_surface_id: "panel".into(),
        prior: ConversationApprovalMode::Ask,
        requested: ConversationApprovalMode::Auto,
        state: ConversationModeRequestState::Pending,
        application: None,
        requested_at_ms: 2,
    };
    opened.store.begin_mode_change(request).await.unwrap();
    opened
        .store
        .observe_mode_application(&id, "watch-mode", ConversationModeApplication::Applied)
        .await
        .unwrap();
    catalogue_watch_pending(&mut watch);
    opened
        .store
        .finish_mode_change(&id, "watch-mode", ConversationModeRequestState::Applied)
        .await
        .unwrap();
    assert_eq!(
        catalogue_watch_ready(&mut watch),
        CatalogueWatchState::Dirty
    );
    opened
        .store
        .finish_mode_change(&id, "watch-mode", ConversationModeRequestState::Applied)
        .await
        .unwrap();
    catalogue_watch_pending(&mut watch);
}

struct PanickingCatalogueWaker;
impl Wake for PanickingCatalogueWaker {
    fn wake(self: Arc<Self>) {
        panic!("test catalogue notification callback unwind");
    }
}

#[tokio::test]
async fn catalogue_watch_callback_panic_cannot_replace_a_durable_create_result() {
    let opened = opened();
    let id = new_id();
    let mut faulty = opened.store.watch(&org(), &alice()).unwrap();
    let mut healthy = opened.store.watch(&org(), &alice()).unwrap();
    let waker = Waker::from(Arc::new(PanickingCatalogueWaker));
    let mut wait = Box::pin(faulty.changed());
    assert!(matches!(
        wait.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));
    let result = opened.store.create(owned(&id)).await;
    assert!(
        ConversationRepository::load(&opened.store, &id)
            .await
            .unwrap()
            .is_some(),
        "metadata really committed"
    );
    assert!(
        result.is_ok(),
        "watch callback changed the durable source result: {result:?}"
    );
    assert_eq!(
        catalogue_watch_ready(&mut healthy),
        CatalogueWatchState::Dirty
    );
    drop(wait);
    assert_eq!(
        catalogue_watch_ready(&mut faulty),
        CatalogueWatchState::NotificationFailed
    );
    drop(healthy);
    let registrations = (1..MAX_CATALOGUE_CHANGE_WATCHES)
        .map(|_| opened.store.watch(&org(), &alice()).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        opened.store.watch(&org(), &alice()),
        Err(CatalogueWatchError::Capacity)
    ));
    drop(registrations);
    drop(opened.store);
    assert_eq!(
        catalogue_watch_ready(&mut faulty),
        CatalogueWatchState::NotificationFailed
    );
}

// Read grants (issue 704): rows G2, G5, G6, G7, G9 and G10 of
// docs/design/read-grants.md.

fn grant_change(
    transition: crate::conversation::application::ShareGrantTransition,
    id: &ConversationId,
    receiver: Option<&str>,
    credential: &str,
    request: &str,
) -> crate::conversation::application::ShareGrantChange {
    crate::conversation::application::ShareGrantChange {
        transition,
        conversation_id: id.clone(),
        principal_id: receiver.map(|id| PrincipalId::new(format!("grantee-{id}")).unwrap()),
        receiver_id: receiver.map(str::to_owned),
        credential_id: nessa_auth::domain::CredentialId::new(credential).unwrap(),
        expected_policy_revision: None,
        initiator: crate::conversation::application::ConversationCaller {
            organization_id: org(),
            principal_id: alice(),
            surface_id: "desktop".into(),
            action_id: request.into(),
        },
        at_ms: 7,
    }
}
async fn grant(store: &LocalConversationStore, id: &ConversationId, receiver: &str) -> bool {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    store
        .change(grant_change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            id,
            Some(receiver),
            &format!("{receiver}-credential"),
            &uuid::Uuid::new_v4().to_string(),
        ))
        .await
        .unwrap()
        .applied
}
async fn revoke(store: &LocalConversationStore, id: &ConversationId, receiver: &str) -> bool {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    store
        .change(grant_change(
            ShareGrantTransition::Revoke,
            id,
            None,
            &format!("{receiver}-credential"),
            &uuid::Uuid::new_v4().to_string(),
        ))
        .await
        .unwrap()
        .applied
}
fn device(receiver: &str) -> Reader {
    Reader::PairedDevice {
        receiver_id: receiver.into(),
    }
}
/// The ids a reader's full pass from `completed` sends, in catalogue order.
async fn device_pass(
    store: &LocalConversationStore,
    reader: Reader,
    completed: u64,
) -> Vec<(ConversationId, u64)> {
    // A pass reaches the head this reader is shown, as a device's does; a
    // reader whose head is not past what it completed has nothing to page.
    let head = store.head(&org(), &alice(), &reader).await.unwrap();
    if head.revision <= completed {
        return Vec::new();
    }
    let mut request = catalogue_request(
        alice(),
        &head.incarnation,
        completed,
        head.revision,
        None,
        MAX_CATALOGUE_ENTRIES,
    );
    request.reader = reader;
    store
        .page(request)
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| (entry.key.id, entry.revision))
        .collect()
}

/// Row G2.
#[tokio::test]
async fn a_device_pages_only_the_conversations_granted_to_it() {
    let opened = opened();
    let (shared, private) = (new_id(), new_id());
    opened.store.create(owned(&shared)).await.unwrap();
    opened.store.create(owned(&private)).await.unwrap();
    assert!(grant(&opened.store, &shared, "phone").await);
    let ids: Vec<_> = device_pass(&opened.store, device("phone"), 0)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, vec![shared.clone()]);
    let owner: Vec<_> = device_pass(&opened.store, Reader::Owner, 0).await;
    assert_eq!(owner.len(), 2, "the owner still pages everything");
    let head = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap();
    for (id, found) in [(&shared, true), (&private, false)] {
        let resolved = opened
            .store
            .resolve(&org(), &alice(), &device("phone"), &head.incarnation, id)
            .await
            .unwrap();
        assert_eq!(resolved.is_some(), found);
    }
    assert!(device_pass(&opened.store, device("tablet"), 0)
        .await
        .is_empty());
}

/// Row G5.
#[tokio::test]
async fn a_grant_moves_the_row_past_what_a_device_completed() {
    let opened = opened();
    let (old, other) = (new_id(), new_id());
    opened.store.create(owned(&old)).await.unwrap();
    opened.store.create(owned(&other)).await.unwrap();
    let completed = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .revision;
    assert!(grant(&opened.store, &old, "phone").await);
    let head = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .revision;
    assert_eq!(head, completed + 1);
    assert_eq!(
        device_pass(&opened.store, device("phone"), completed).await,
        vec![(old, head)]
    );
}

/// Row G6.
#[tokio::test]
async fn a_revoke_takes_the_row_out_of_the_devices_catalogue() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    grant(&opened.store, &id, "phone").await;
    let before = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .revision;
    assert!(revoke(&opened.store, &id, "phone").await);
    let head = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap();
    assert_eq!(head.revision, before + 1, "a revoke moves the head");
    assert!(device_pass(&opened.store, device("phone"), 0)
        .await
        .is_empty());
    assert!(opened
        .store
        .resolve(&org(), &alice(), &device("phone"), &head.incarnation, &id)
        .await
        .unwrap()
        .is_none());
    use crate::conversation::application::ShareGrants;
    assert!(!opened.store.is_granted(&id, "phone").await.unwrap());
}

/// A version 4 file, whose grants hold no capability set but read and no
/// policy, is refused rather than read into the version 6 shape, and keeps
/// its rows (docs/adr/todo/202-versioned-local-datasets.md).
#[test]
fn a_version_4_database_is_refused_without_erasing_its_grants() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let old = nessa_local_database::Schema::new(
        "CREATE TABLE read_grants (receiver_id TEXT NOT NULL, role TEXT NOT NULL CHECK (role = 'read')) STRICT;\nPRAGMA user_version = 4;",
    )
    .unwrap();
    let connection = nessa_local_database::open(&path, &old).unwrap();
    connection
        .execute(
            "INSERT INTO read_grants (receiver_id, role) VALUES ('phone', 'read')",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        LocalConversationStore::open(&path),
        Err(nessa_local_database::OpenError::Version {
            found: 4,
            expected: 6
        })
    ));
    let kept: String = raw(&path)
        .query_row("SELECT receiver_id FROM read_grants", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, "phone");
}

/// The previous metadata version, whose grants name a role, is refused and
/// left untouched. This build reads version 6 only.
#[test]
fn a_version_5_database_is_refused_without_erasing_its_grants() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let old = nessa_local_database::Schema::new(
        "CREATE TABLE share_grants (
            receiver_id TEXT NOT NULL,
            role TEXT NOT NULL CHECK (role IN ('read', 'comment', 'drive'))
         ) STRICT;
         PRAGMA user_version = 5;",
    )
    .unwrap();
    let connection = nessa_local_database::open(&path, &old).unwrap();
    connection
        .execute(
            "INSERT INTO share_grants (receiver_id, role) VALUES ('phone', 'drive')",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        LocalConversationStore::open(&path),
        Err(nessa_local_database::OpenError::Version {
            found: 5,
            expected: 6
        })
    ));
    let kept: String = raw(&path)
        .query_row("SELECT role FROM share_grants", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, "drive");
}

/// A comment grant written through the store, past `conversation.share`, is
/// a legal capability set. Cedar lets the device that is signed in as the
/// binding owner comment, and denies the gateway comment because it is not.
/// Neither may drive: the set does not contain `drive`.
#[tokio::test]
async fn a_hand_built_comment_grant_for_a_gateway_is_denied_by_cedar() {
    use crate::conversation::application::{
        AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
        ConversationAccessPolicyEvaluator, RequestShape, ShareGrants,
    };
    use crate::conversation::infrastructure::CedarConversationAccessPolicy;
    let opened = opened();
    let id = new_id();
    let record = owned(&id);
    opened.store.create(record.clone()).await.unwrap();
    let comment = ShareAccess::new(Capabilities::comment(), None).unwrap();
    assert!(share_as(&opened.store, &id, "peer", comment, "hand")
        .await
        .unwrap());
    let held = opened.store.held_capabilities(&id, "peer").await.unwrap();
    assert_eq!(held, Some(Capabilities::comment()));
    let policy = CedarConversationAccessPolicy::new().unwrap();
    let (organization, owner, gateway) =
        (org(), alice(), PrincipalId::new("gateway:peer").unwrap());
    let ask = |signed_in_as: &PrincipalId, action: AccessAction| {
        policy.evaluate(&AccessAsk {
            subject: AccessSubject::PairedReader {
                organization: &organization,
                signed_in_as,
                binding_owner: &owner,
                receiver: "peer",
            },
            action,
            resource: AccessResource::of(&record),
            held,
            request: RequestShape::default(),
            target: None,
        })
    };
    assert!(matches!(
        ask(&gateway, AccessAction::Read),
        AccessDecision::Allow { .. }
    ));
    assert!(matches!(
        ask(&owner, AccessAction::Comment),
        AccessDecision::Allow { .. }
    ));
    assert_eq!(
        ask(&gateway, AccessAction::Comment),
        AccessDecision::Deny(AccessDenial::NotPermitted)
    );
    assert_eq!(
        ask(&owner, AccessAction::Drive),
        AccessDecision::Deny(AccessDenial::NotPermitted)
    );
    assert_eq!(
        ask(&gateway, AccessAction::Drive),
        AccessDecision::Deny(AccessDenial::NotPermitted)
    );
}

/// A grant is keyed by principal, resource kind, and resource id. This slice
/// writes kind `conversation`. One may name no receiver, and is not found by
/// a receiver lookup. A paired grant is still found by its receiver. A second
/// row for the same principal, kind, and resource id cannot be stored: a later
/// change of that principal replaces the row it has. The same principal and
/// resource id with kind `directory` is a different row.
#[tokio::test]
async fn a_grant_is_keyed_by_principal_and_may_name_no_receiver() {
    use crate::conversation::application::{
        ResourceKind, ShareGrantChange, ShareGrantTransition, ShareGrants,
    };
    use nessa_auth::domain::CredentialId;
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let principal = PrincipalId::new("webhook:one").unwrap();
    let caller = |request: &str| crate::conversation::application::ConversationCaller {
        organization_id: org(),
        principal_id: alice(),
        surface_id: "desktop".into(),
        action_id: request.into(),
    };
    let stored = opened
        .store
        .change(ShareGrantChange {
            transition: ShareGrantTransition::Grant(ShareAccess::read()),
            conversation_id: id.clone(),
            principal_id: Some(principal.clone()),
            receiver_id: None,
            credential_id: CredentialId::new("webhook-credential").unwrap(),
            expected_policy_revision: None,
            initiator: caller("grant-webhook"),
            at_ms: 7,
        })
        .await
        .unwrap();
    assert!(stored.applied);
    assert_eq!(
        opened.store.held_grant(&id, "anyone").await.unwrap(),
        None,
        "a grant with no receiver is not found by one"
    );
    let listed = opened.store.grants(&id).await.unwrap().grants;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].principal_id, principal);
    assert_eq!(listed[0].receiver_id, None);

    assert!(grant(&opened.store, &id, "phone").await);
    assert!(opened
        .store
        .held_grant(&id, "phone")
        .await
        .unwrap()
        .is_some());
    let replaced = opened
        .store
        .change(ShareGrantChange {
            transition: ShareGrantTransition::Grant(
                ShareAccess::new(Capabilities::comment(), None).unwrap(),
            ),
            conversation_id: id.clone(),
            principal_id: Some(PrincipalId::new("grantee-phone").unwrap()),
            receiver_id: Some("tablet".into()),
            credential_id: CredentialId::new("tablet-credential").unwrap(),
            expected_policy_revision: None,
            initiator: caller("replace-phone"),
            at_ms: 8,
        })
        .await
        .unwrap();
    assert!(replaced.applied);
    let grants = opened.store.grants(&id).await.unwrap().grants;
    assert_eq!(grants.len(), 2, "the webhook and the one paired principal");
    let paired = grants
        .iter()
        .find(|grant| grant.principal_id.as_str() == "grantee-phone")
        .expect("the paired principal");
    assert_eq!(paired.receiver_id.as_deref(), Some("tablet"));
    assert_eq!(paired.credential_id.as_str(), "tablet-credential");
    assert_eq!(paired.access.capabilities(), Capabilities::comment());
    assert!(opened
        .store
        .held_grant(&id, "phone")
        .await
        .unwrap()
        .is_none());
    assert!(opened
        .store
        .held_grant(&id, "tablet")
        .await
        .unwrap()
        .is_some());
    let kind: String = raw(&opened.path)
        .query_row(
            "SELECT resource_kind FROM share_grants WHERE principal_id = 'webhook:one'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, ResourceKind::Conversation.as_str());
    let duplicated = raw(&opened.path).execute(
        "INSERT INTO share_grants
            (principal_id, resource_kind, resource_id, receiver_id, credential_id, capabilities,
             policy, policy_revision, granted_at_ms)
         SELECT principal_id, resource_kind, resource_id, 'other', 'other-credential',
                capabilities, policy, policy_revision, granted_at_ms
         FROM share_grants WHERE principal_id = 'grantee-phone'",
        [],
    );
    let error = duplicated
        .expect_err("the primary key refuses a second row")
        .to_string();
    assert!(
        error.contains("UNIQUE"),
        "the primary key refuses a second row for one principal and resource: {error}"
    );
    let directory = ResourceKind::Directory.as_str();
    raw(&opened.path)
        .execute(
            "INSERT INTO share_grants
                (principal_id, resource_kind, resource_id, receiver_id, credential_id, capabilities,
                 policy, policy_revision, granted_at_ms)
             SELECT principal_id, ?1, resource_id, NULL, 'dir-credential', capabilities, policy,
                    policy_revision, granted_at_ms
             FROM share_grants WHERE principal_id = 'grantee-phone'",
            [directory],
        )
        .expect("a different resource kind is a different grant");
    assert_eq!(
        opened.store.grants(&id).await.unwrap().grants.len(),
        2,
        "a directory row is not a conversation grant"
    );
    let again = raw(&opened.path).execute(
        "INSERT INTO share_grants
            (principal_id, resource_kind, resource_id, receiver_id, credential_id, capabilities,
             policy, policy_revision, granted_at_ms)
         SELECT principal_id, resource_kind, resource_id, NULL, 'dir-credential-2', capabilities,
                policy, policy_revision, granted_at_ms
         FROM share_grants WHERE resource_kind = 'directory'",
        [],
    );
    let error = again
        .expect_err("the primary key refuses a second directory row")
        .to_string();
    assert!(
        error.contains("UNIQUE"),
        "the primary key refuses a second row for one principal, kind, and resource: {error}"
    );
}

/// Row H9 (`docs/design/auth/peer-gateways.md`): a paired reader's head
/// moves only with what it was granted. An owner change to a row it was not
/// granted, or a grant to another receiver, leaves it where it was; a grant,
/// a change to a granted row and a revoke each move it, and a revoke moves it
/// forward, never back to an older granted row.
#[tokio::test]
async fn a_paired_readers_head_moves_only_with_what_it_was_granted() {
    let opened = opened();
    let store = &opened.store;
    let heads = || async {
        let head = |reader: Reader| async move {
            store
                .head(&org(), &alice(), &reader)
                .await
                .unwrap()
                .revision
        };
        (head(Reader::Owner).await, head(device("phone")).await)
    };
    let (first, second) = (new_id(), new_id());
    store.create(owned(&first)).await.unwrap();
    store.create(owned(&second)).await.unwrap();
    assert_eq!(heads().await.1, 0, "nothing granted yet");
    assert!(grant(store, &first, "phone").await);
    let (owner, granted) = heads().await;
    assert_eq!(granted, owner, "a grant moves it");
    // Owner work it was not granted, and a grant to another receiver.
    let third = new_id();
    store.create(owned(&third)).await.unwrap();
    assert!(grant(store, &second, "tablet").await);
    let (owner, unmoved) = heads().await;
    assert!(owner > granted);
    assert_eq!(unmoved, granted, "an ungranted change does not move it");
    assert!(grant(store, &second, "phone").await);
    store
        .record_deletion(&second, deletion("delete"))
        .await
        .unwrap();
    let (owner, changed) = heads().await;
    assert!(changed > unmoved);
    assert_eq!(changed, owner, "a change to a granted row moves it");
    // Revoking the row that holds the highest revision must not drop the
    // head back to the other granted row's.
    assert!(revoke(store, &second, "phone").await);
    let (owner, revoked) = heads().await;
    assert!(revoked > changed, "a revoke moves it forward");
    assert_eq!(revoked, owner);
    assert!(revoke(store, &first, "phone").await);
    let (_, empty) = heads().await;
    assert!(
        empty > revoked,
        "revoking the last grant still moves it forward"
    );
}

/// Row G7.
#[tokio::test]
async fn a_grant_names_one_conversation_and_nothing_else() {
    use crate::conversation::application::ShareGrants;
    let opened = opened();
    let (shared, existing) = (new_id(), new_id());
    opened.store.create(owned(&shared)).await.unwrap();
    opened.store.create(owned(&existing)).await.unwrap();
    grant(&opened.store, &shared, "phone").await;
    let later = new_id();
    opened
        .store
        .create(owned_by(&later, "org", "alice", 9))
        .await
        .unwrap();
    for id in [&existing, &later] {
        assert!(!opened.store.is_granted(id, "phone").await.unwrap());
    }
    assert!(opened.store.is_granted(&shared, "phone").await.unwrap());
}

/// Row G9.
#[tokio::test]
async fn a_repeated_share_or_unshare_changes_nothing() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    assert!(!revoke(&opened.store, &id, "phone").await);
    assert!(grant(&opened.store, &id, "phone").await);
    let head = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .revision;
    assert!(!grant(&opened.store, &id, "phone").await);
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        head
    );
    let unchanged: i64 = raw(&opened.path)
        .query_row(
            "SELECT COUNT(*) FROM share_grant_changes WHERE before = after",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, 2);
}

/// Row G16, for a request that changed nothing: a share of a conversation
/// already shared, its reply lost and retried after an unshare, still answers
/// `applied: false` and does not grant again.
#[tokio::test]
async fn a_retried_share_that_changed_nothing_never_undoes_a_later_unshare() {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    assert!(grant(&opened.store, &id, "phone").await);
    let share = || {
        grant_change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            &id,
            Some("phone"),
            "phone-credential",
            "share-again",
        )
    };
    assert!(!opened.store.change(share()).await.unwrap().applied);
    assert!(revoke(&opened.store, &id, "phone").await);
    let head = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .revision;
    assert!(!opened.store.change(share()).await.unwrap().applied);
    assert!(!opened.store.is_granted(&id, "phone").await.unwrap());
    assert_eq!(
        opened
            .store
            .head(
                &org(),
                &alice(),
                &crate::conversation::application::Reader::Owner
            )
            .await
            .unwrap()
            .revision,
        head
    );
}

/// Row G16: a retried request answers what it answered the first time. A
/// share whose reply was lost, retried after a later unshare, does not grant
/// again; the same request naming another device or change is refused.
#[tokio::test]
async fn a_retried_share_replays_its_answer_and_never_undoes_a_later_unshare() {
    use crate::conversation::application::{ConversationError, ShareGrantTransition, ShareGrants};
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let share = || {
        grant_change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            &id,
            Some("phone"),
            "phone-credential",
            "share-1",
        )
    };
    assert!(opened.store.change(share()).await.unwrap().applied);
    assert!(revoke(&opened.store, &id, "phone").await);
    assert!(opened.store.change(share()).await.unwrap().applied);
    assert!(!opened.store.is_granted(&id, "phone").await.unwrap());
    for (transition, credential) in [
        (ShareGrantTransition::Revoke, "phone-credential"),
        (
            ShareGrantTransition::Grant(ShareAccess::read()),
            "tablet-credential",
        ),
    ] {
        let refused = opened
            .store
            .change(grant_change(
                transition,
                &id,
                Some("tablet"),
                credential,
                "share-1",
            ))
            .await;
        assert!(
            matches!(refused, Err(ConversationError::InvalidInput)),
            "{:?}",
            refused.map_err(|error| error.to_string())
        );
    }
    let journaled: i64 = raw(&opened.path)
        .query_row("SELECT COUNT(*) FROM share_grant_changes", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(journaled, 2);
}

/// Row G17: a conversation holds at most
/// `MAX_SHARE_GRANTS_PER_CONVERSATION` grants, so its shares answer fits one
/// frame; one more is refused and writes nothing.
#[tokio::test]
async fn a_conversation_holds_a_bounded_number_of_grants() {
    use crate::conversation::application::{
        ConversationError, ShareGrantTransition, ShareGrants, MAX_SHARE_GRANTS_PER_CONVERSATION,
    };
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    for device in 0..MAX_SHARE_GRANTS_PER_CONVERSATION {
        assert!(grant(&opened.store, &id, &format!("device-{device}")).await);
    }
    let refused = opened
        .store
        .change(grant_change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            &id,
            Some("one-more"),
            "one-more-credential",
            "one-more",
        ))
        .await;
    assert!(matches!(refused, Err(ConversationError::InvalidInput)));
    assert!(!opened.store.is_granted(&id, "one-more").await.unwrap());
    assert_eq!(
        opened.store.grants(&id).await.unwrap().grants.len() as i64,
        MAX_SHARE_GRANTS_PER_CONVERSATION
    );
}

/// Row G8, in the store: a grant or revoke on another owner's conversation
/// is refused as not found inside the transaction that would write it, and
/// writes nothing.
#[tokio::test]
async fn a_grant_on_another_owners_conversation_writes_nothing() {
    use crate::conversation::application::{ConversationError, ShareGrantTransition, ShareGrants};
    let opened = opened();
    let id = new_id();
    opened
        .store
        .create(owned_by(&id, "org", "bob", 1))
        .await
        .unwrap();
    for transition in [
        ShareGrantTransition::Grant(ShareAccess::read()),
        ShareGrantTransition::Revoke,
    ] {
        let refused = opened
            .store
            .change(grant_change(
                transition,
                &id,
                Some("phone"),
                "phone-credential",
                "share",
            ))
            .await;
        assert!(
            matches!(refused, Err(ConversationError::NotFound)),
            "{:?}",
            refused.map_err(|error| error.to_string())
        );
    }
    let written: i64 = raw(&opened.path)
        .query_row(
            "SELECT (SELECT COUNT(*) FROM share_grants) + (SELECT COUNT(*) FROM share_grant_changes)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(written, 0);
}

/// One `share_grant_changes` row, in column order.
type JournalRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
);

/// Row G10.
#[tokio::test]
async fn every_grant_change_is_journaled_with_its_initiator() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    {
        use crate::conversation::application::{ShareGrantTransition, ShareGrants};
        for (transition, receiver, request) in [
            (
                ShareGrantTransition::Grant(ShareAccess::read()),
                Some("phone"),
                "share",
            ),
            (ShareGrantTransition::Revoke, None, "unshare"),
        ] {
            assert!(
                opened
                    .store
                    .change(grant_change(
                        transition,
                        &id,
                        receiver,
                        "phone-credential",
                        request
                    ))
                    .await
                    .unwrap()
                    .applied
            );
        }
    }
    let head = opened
        .store
        .head(
            &org(),
            &alice(),
            &crate::conversation::application::Reader::Owner,
        )
        .await
        .unwrap()
        .revision;
    let connection = raw(&opened.path);
    let mut statement = connection
        .prepare(
            "SELECT resource_id, receiver_id, credential_id, before, after, initiator,
                    surface, request, changed_at_ms, revision
             FROM share_grant_changes ORDER BY sequence",
        )
        .unwrap();
    let rows: Vec<JournalRow> = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let row = |before: &str, after: &str, request: &str, revision: u64| {
        (
            id.to_string(),
            "phone".to_owned(),
            "phone-credential".to_owned(),
            before.to_owned(),
            after.to_owned(),
            "alice".to_owned(),
            "desktop".to_owned(),
            request.to_owned(),
            7,
            i64::try_from(revision).unwrap(),
        )
    };
    assert_eq!(
        rows,
        vec![
            row("none", "read", "share", head - 1),
            row("read", "none", "unshare", head),
        ]
    );
}

// Roles and tool policies (issue 707): rows J1, J4, J5, J6, J8 and J9 of
// docs/design/read-grants.md.

fn drive_with(policy: ToolPolicy) -> ShareAccess {
    ShareAccess::new(Capabilities::drive(), Some(policy)).unwrap()
}
fn no_shell() -> ShareAccess {
    drive_with(ToolPolicy::preset(ToolPreset::NoShell))
}
fn rules() -> ShareAccess {
    drive_with(ToolPolicy::rules(
        ToolRules::new(vec!["Read".into()], vec!["Bash(rm -rf:*)".into()]).unwrap(),
    ))
}
/// Share `receiver` `access` on `id` under request `request`.
async fn share_as(
    store: &LocalConversationStore,
    id: &ConversationId,
    receiver: &str,
    access: ShareAccess,
    request: &str,
) -> Result<bool, crate::conversation::application::ConversationError> {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    store
        .change(grant_change(
            ShareGrantTransition::Grant(access),
            id,
            Some(receiver),
            &format!("{receiver}-credential"),
            request,
        ))
        .await
        .map(|receipt| receipt.applied)
}
/// The one grant on `id`.
async fn only_grant(
    store: &LocalConversationStore,
    id: &ConversationId,
) -> crate::conversation::application::ShareGrant {
    use crate::conversation::application::ShareGrants;
    let mut grants = store.grants(id).await.unwrap().grants;
    assert_eq!(grants.len(), 1);
    grants.remove(0)
}
/// Each journal row: role and policy before and after, its revision and its
/// sequence.
type RoleRow = (String, String, Option<String>, Option<String>, i64, i64);
fn role_journal(path: &Path) -> Vec<RoleRow> {
    raw(path)
        .prepare(
            "SELECT before, after, policy_before, policy_after, revision, sequence
             FROM share_grant_changes ORDER BY sequence",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
async fn owner_head(store: &LocalConversationStore) -> u64 {
    store
        .head(&org(), &alice(), &Reader::Owner)
        .await
        .unwrap()
        .revision
}

/// Row J1, in the catalogue: a Comment or a Drive grant pages, resolves and
/// moves the device's head exactly as a Read grant does.
#[tokio::test]
async fn a_comment_or_drive_grant_pages_and_resolves_as_read_does() {
    use crate::conversation::application::ShareGrants;
    let seen = |access: ShareAccess| async move {
        let opened = opened();
        let (shared, private) = (new_id(), new_id());
        opened.store.create(owned(&shared)).await.unwrap();
        opened.store.create(owned(&private)).await.unwrap();
        assert!(share_as(&opened.store, &shared, "phone", access, "share")
            .await
            .unwrap());
        let pass = device_pass(&opened.store, device("phone"), 0).await;
        let head = opened
            .store
            .head(&org(), &alice(), &device("phone"))
            .await
            .unwrap();
        let mut resolved = Vec::new();
        for id in [&shared, &private] {
            resolved.push(
                opened
                    .store
                    .resolve(&org(), &alice(), &device("phone"), &head.incarnation, id)
                    .await
                    .unwrap()
                    .is_some(),
            );
        }
        let granted = opened.store.is_granted(&shared, "phone").await.unwrap();
        (
            pass.into_iter()
                .map(|(id, revision)| (id == shared, revision))
                .collect::<Vec<_>>(),
            head.revision,
            resolved,
            granted,
        )
    };
    let read = seen(ShareAccess::read()).await;
    assert_eq!(read, (vec![(true, 3)], 3, vec![true, false], true));
    for access in crate::conversation_test_support::roles_beyond_read() {
        assert_eq!(seen(access.clone()).await, read, "{access:?}");
    }
}

/// Row J4: sharing again with another role or policy replaces them. It is
/// applied and journaled with the role and policy before and after; the
/// grant's policy revision moves to that journal row; the catalogue revision
/// and the device's head stay where they were, because what it reads did
/// not change.
#[tokio::test]
async fn a_change_of_role_or_policy_is_applied_journaled_and_moves_the_policy_revision() {
    use crate::conversation::application::ShareGrants;
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    assert!(share_as(store, &id, "phone", ShareAccess::read(), "share")
        .await
        .unwrap());
    let catalogue = store.grants(&id).await.unwrap().change_revision;
    let read = only_grant(store, &id).await;
    let head = owner_head(store).await;
    let device_head = store
        .head(&org(), &alice(), &device("phone"))
        .await
        .unwrap()
        .revision;
    assert!(share_as(store, &id, "phone", no_shell(), "to-drive")
        .await
        .unwrap());
    let driving = only_grant(store, &id).await;
    assert_eq!(driving.access, no_shell());
    assert!(driving.policy_revision > read.policy_revision);
    assert_eq!(driving.granted_at_ms, read.granted_at_ms);
    assert!(share_as(store, &id, "phone", rules(), "to-rules")
        .await
        .unwrap());
    let ruled = only_grant(store, &id).await;
    assert_eq!(ruled.access, rules());
    assert!(ruled.policy_revision > driving.policy_revision);
    assert!(
        share_as(store, &id, "phone", ShareAccess::read(), "to-read")
            .await
            .unwrap()
    );
    let back = only_grant(store, &id).await;
    assert_eq!(back.access, ShareAccess::read());
    assert!(back.policy_revision > ruled.policy_revision);
    assert_eq!(owner_head(store).await, head, "no catalogue revision moved");
    assert_eq!(
        store.grants(&id).await.unwrap().change_revision,
        catalogue,
        "a policy change leaves the revision a shares page is bound to"
    );
    assert_eq!(
        store
            .head(&org(), &alice(), &device("phone"))
            .await
            .unwrap()
            .revision,
        device_head
    );
    let journal = role_journal(&opened.path);
    let no_shell_text = Some(r#"{"preset":"no_shell"}"#.to_owned());
    let rules_text = Some(r#"{"allow":["Read"],"deny":["Bash(rm -rf:*)"]}"#.to_owned());
    let revision = i64::try_from(head).unwrap();
    let row = |before: &str,
               after: &str,
               policy_before: &Option<String>,
               policy_after: &Option<String>| {
        (
            before.to_owned(),
            after.to_owned(),
            policy_before.clone(),
            policy_after.clone(),
        )
    };
    assert_eq!(
        journal
            .iter()
            .map(|(before, after, policy_before, policy_after, _, _)| row(
                before,
                after,
                policy_before,
                policy_after
            ))
            .collect::<Vec<_>>(),
        vec![
            row("none", "read", &None, &None),
            row("read", "read+comment+drive", &None, &no_shell_text),
            row(
                "read+comment+drive",
                "read+comment+drive",
                &no_shell_text,
                &rules_text
            ),
            row("read+comment+drive", "read", &rules_text, &None),
        ]
    );
    assert!(journal.iter().all(|entry| entry.4 == revision));
    let sequences: Vec<u64> = journal
        .iter()
        .map(|entry| u64::try_from(entry.5).unwrap())
        .collect();
    assert_eq!(
        sequences,
        vec![
            read.policy_revision,
            driving.policy_revision,
            ruled.policy_revision,
            back.policy_revision
        ]
    );
}

/// Row J5: sharing the role and policy already held changes nothing: not
/// applied, journaled with before equal to after, the policy revision kept.
#[tokio::test]
async fn an_identical_share_changes_nothing_and_keeps_the_policy_revision() {
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    assert!(share_as(store, &id, "phone", rules(), "share")
        .await
        .unwrap());
    let first = only_grant(store, &id).await;
    let head = owner_head(store).await;
    assert!(!share_as(store, &id, "phone", rules(), "again")
        .await
        .unwrap());
    assert_eq!(only_grant(store, &id).await, first);
    assert_eq!(owner_head(store).await, head);
    let journal = role_journal(&opened.path);
    assert_eq!(journal.len(), 2);
    let (before, after, policy_before, policy_after, _, _) = &journal[1];
    assert_eq!((before, policy_before), (after, policy_after));
    assert_eq!(after, "read+comment+drive");
}

/// Two paired credentials of one principal are one grant. Sharing the same
/// capabilities through the other receiver applies, keeps the grant epoch,
/// and moves the catalogue: the receiver that gained the grant pages the
/// conversation, and the one that lost it drops it. A retry of that request
/// answers the same receipt.
#[tokio::test]
async fn a_receiver_switch_is_applied_and_moves_the_catalogue() {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    let principal = PrincipalId::new("grantee-shared").unwrap();
    let share = |receiver: &str, request: &str| {
        let mut change = grant_change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            &id,
            Some(receiver),
            &format!("{receiver}-credential"),
            request,
        );
        change.principal_id = Some(principal.clone());
        change
    };
    let first = store.change(share("phone", "share")).await.unwrap();
    assert!(first.applied);
    let epoch = only_grant(store, &id).await.policy_revision;
    assert_eq!(first.policy_revision, Some(epoch));
    let phone_head = store
        .head(&org(), &alice(), &device("phone"))
        .await
        .unwrap()
        .revision;
    let catalogue = store.grants(&id).await.unwrap().change_revision;
    let switched = store.change(share("tablet", "switch")).await.unwrap();
    assert!(switched.applied);
    assert_eq!(switched.policy_revision, Some(epoch));
    let grant = only_grant(store, &id).await;
    assert_eq!(grant.principal_id, principal);
    assert_eq!(grant.receiver_id.as_deref(), Some("tablet"));
    assert_eq!(grant.credential_id.as_str(), "tablet-credential");
    assert_eq!(grant.policy_revision, epoch);
    assert!(store.grants(&id).await.unwrap().change_revision > catalogue);
    assert!(!store.is_granted(&id, "phone").await.unwrap());
    assert!(store.is_granted(&id, "tablet").await.unwrap());
    let phone_after = store
        .head(&org(), &alice(), &device("phone"))
        .await
        .unwrap()
        .revision;
    assert!(phone_after > phone_head);
    assert!(device_pass(store, device("phone"), phone_head)
        .await
        .is_empty());
    assert_eq!(
        device_pass(store, device("tablet"), phone_head).await,
        vec![(id.clone(), phone_after)]
    );
    assert_eq!(
        store.change(share("tablet", "switch")).await.unwrap(),
        switched
    );
    let journal = role_journal(&opened.path);
    assert_eq!(journal.len(), 2);
    assert_eq!(journal[1].0, "read");
    assert_eq!(journal[1].1, "read");
}

/// A change that names a principal resolves that principal's row only. Naming
/// another principal's credential does not rewrite that grant.
#[tokio::test]
async fn a_named_principal_does_not_take_another_principals_grant() {
    use crate::conversation::application::{ConversationError, ShareGrantTransition, ShareGrants};
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    assert!(grant(store, &id, "phone").await);
    let held = only_grant(store, &id).await;
    let mut change = grant_change(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &id,
        Some("tablet"),
        "phone-credential",
        "take",
    );
    change.principal_id = Some(PrincipalId::new("grantee-other").unwrap());
    assert!(matches!(
        store.change(change).await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(only_grant(store, &id).await, held);
}

/// A retry is the first request. Naming another principal, or another
/// receiver, under that request id is refused and leaves the grant and the
/// journal as the first write left them. The same refusal holds for a revoke.
#[tokio::test]
async fn a_retried_share_that_names_another_principal_is_refused() {
    use crate::conversation::application::{ConversationError, ShareGrantTransition, ShareGrants};
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    let first = grant_change(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &id,
        Some("phone"),
        "phone-credential",
        "share",
    );
    let granted = store.change(first.clone()).await.unwrap();
    assert!(granted.applied);
    assert_eq!(store.change(first).await.unwrap(), granted);
    let held = only_grant(store, &id).await;
    let journaled = role_journal(&opened.path).len();
    let mut other_principal = grant_change(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &id,
        Some("phone"),
        "phone-credential",
        "share",
    );
    other_principal.principal_id = Some(PrincipalId::new("grantee-other").unwrap());
    assert!(matches!(
        store.change(other_principal).await,
        Err(ConversationError::InvalidInput)
    ));
    let mut other_receiver = grant_change(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &id,
        Some("tablet"),
        "phone-credential",
        "share",
    );
    other_receiver.principal_id = Some(held.principal_id.clone());
    assert!(matches!(
        store.change(other_receiver).await,
        Err(ConversationError::InvalidInput)
    ));
    assert_eq!(only_grant(store, &id).await, held);
    assert_eq!(role_journal(&opened.path).len(), journaled);

    let revoke = grant_change(
        ShareGrantTransition::Revoke,
        &id,
        None,
        "phone-credential",
        "revoke",
    );
    let revoked = store.change(revoke.clone()).await.unwrap();
    assert!(revoked.applied);
    assert_eq!(store.change(revoke).await.unwrap(), revoked);
    let journaled = role_journal(&opened.path).len();
    let mut revoked_as_other = grant_change(
        ShareGrantTransition::Revoke,
        &id,
        None,
        "phone-credential",
        "revoke",
    );
    revoked_as_other.principal_id = Some(PrincipalId::new("grantee-other").unwrap());
    assert!(matches!(
        store.change(revoked_as_other).await,
        Err(ConversationError::InvalidInput)
    ));
    let mut revoked_for_tablet = grant_change(
        ShareGrantTransition::Revoke,
        &id,
        Some("tablet"),
        "phone-credential",
        "revoke",
    );
    revoked_for_tablet.principal_id = None;
    assert!(matches!(
        store.change(revoked_for_tablet).await,
        Err(ConversationError::InvalidInput)
    ));
    assert!(!store.is_granted(&id, "phone").await.unwrap());
    assert!(!store.is_granted(&id, "tablet").await.unwrap());
    assert_eq!(role_journal(&opened.path).len(), journaled);
}

/// Moving a drive grant onto another receiver keeps the epoch, and that
/// receiver still reads the policy the epoch names. The receiver that lost
/// the grant is a revocation, so a snapshot cannot give it back.
#[tokio::test]
async fn a_replaced_receiver_keeps_the_drive_policy_and_is_fenced() {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    let principal = PrincipalId::new("grantee-shared").unwrap();
    let share = |receiver: &str, request: &str| {
        let mut change = grant_change(
            ShareGrantTransition::Grant(no_shell()),
            &id,
            Some(receiver),
            &format!("{receiver}-credential"),
            request,
        );
        change.principal_id = Some(principal.clone());
        change
    };
    assert!(store.change(share("phone", "drive")).await.unwrap().applied);
    let epoch = only_grant(store, &id).await.policy_revision;
    let moved = store.change(share("tablet", "move")).await.unwrap();
    assert!(moved.applied);
    assert_eq!(moved.policy_revision, Some(epoch));
    assert_eq!(
        store
            .policy_at(&id, "tablet", epoch)
            .await
            .unwrap()
            .as_ref(),
        no_shell().policy()
    );
    assert_eq!(
        store.policy_at(&id, "phone", epoch).await.unwrap().as_ref(),
        no_shell().policy()
    );
    assert_eq!(store.policy_at(&id, "laptop", epoch).await.unwrap(), None);
    let facts = store.export_facts().await.unwrap();
    assert_eq!(facts.revocations.len(), 1);
    let revocation = &facts.revocations[0];
    assert_eq!(revocation.receiver_id, "phone");
    assert_eq!(revocation.credential_id, "phone-credential");
    assert_eq!(revocation.capabilities, "read+comment+drive");
}

/// Row J6: a share retried with its request id answers what it answered the
/// first time and changes nothing, even after a later change of role; the
/// same request naming another role or another policy is refused.
#[tokio::test]
async fn a_retried_share_replays_its_role_and_policy_or_is_refused() {
    use crate::conversation::application::ConversationError;
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    assert!(share_as(store, &id, "phone", no_shell(), "share-1")
        .await
        .unwrap());
    assert!(
        share_as(store, &id, "phone", ShareAccess::read(), "share-2")
            .await
            .unwrap()
    );
    assert!(share_as(store, &id, "phone", no_shell(), "share-1")
        .await
        .unwrap());
    assert_eq!(only_grant(store, &id).await.access, ShareAccess::read());
    let comment = ShareAccess::new(Capabilities::comment(), None).unwrap();
    for other in [comment, rules(), ShareAccess::read()] {
        let refused = share_as(store, &id, "phone", other.clone(), "share-1").await;
        assert!(
            matches!(refused, Err(ConversationError::InvalidInput)),
            "{other:?}: {:?}",
            refused.map_err(|error| error.to_string())
        );
    }
    assert_eq!(role_journal(&opened.path).len(), 2);
    assert_eq!(only_grant(store, &id).await.access, ShareAccess::read());
}

/// Apply `transition` to `receiver`'s grant on `id` under request
/// `request`, built on the grant at `expected` when it names one.
async fn change_from(
    store: &LocalConversationStore,
    id: &ConversationId,
    receiver: &str,
    transition: crate::conversation::application::ShareGrantTransition,
    request: &str,
    expected: Option<u64>,
) -> Result<
    crate::conversation::application::ShareGrantReceipt,
    crate::conversation::application::ConversationError,
> {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    let named = match transition {
        ShareGrantTransition::Grant(_) => Some(receiver),
        ShareGrantTransition::Revoke => None,
    };
    store
        .change(crate::conversation::application::ShareGrantChange {
            expected_policy_revision: expected,
            ..grant_change(
                transition,
                id,
                named,
                &format!("{receiver}-credential"),
                request,
            )
        })
        .await
}

/// Row J29: a share answers the policy revision the grant holds once it is
/// made — the new one when applied, the one kept when not — and a revoke
/// names none. A retried request answers what its first answer did, after
/// later changes too, and a grant made after a revoke takes a revision past
/// every one before it.
#[tokio::test]
async fn a_share_answers_the_policy_revision_it_left_and_a_retry_answers_the_same() {
    use crate::conversation::application::{ShareGrantReceipt, ShareGrantTransition};
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    let grant = |access: ShareAccess| ShareGrantTransition::Grant(access);
    let read = change_from(store, &id, "phone", grant(ShareAccess::read()), "s1", None)
        .await
        .unwrap();
    let first = only_grant(store, &id).await.policy_revision;
    assert_eq!(
        read,
        ShareGrantReceipt {
            applied: true,
            policy_revision: Some(first)
        }
    );
    let again = change_from(store, &id, "phone", grant(ShareAccess::read()), "s2", None)
        .await
        .unwrap();
    assert_eq!(
        again,
        ShareGrantReceipt {
            applied: false,
            policy_revision: Some(first)
        }
    );
    let driving = change_from(store, &id, "phone", grant(no_shell()), "s3", None)
        .await
        .unwrap();
    let second = only_grant(store, &id).await.policy_revision;
    assert!(second > first);
    assert_eq!(driving.policy_revision, Some(second));
    // Each retry answers its first answer, not what the grant holds now.
    for (request, access, answered) in [
        ("s1", ShareAccess::read(), read),
        ("s2", ShareAccess::read(), again),
        ("s3", no_shell(), driving),
    ] {
        assert_eq!(
            change_from(store, &id, "phone", grant(access), request, None)
                .await
                .unwrap(),
            answered,
            "{request}"
        );
    }
    let revoked = change_from(
        store,
        &id,
        "phone",
        ShareGrantTransition::Revoke,
        "r1",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        revoked,
        ShareGrantReceipt {
            applied: true,
            policy_revision: None
        }
    );
    let anew = change_from(store, &id, "phone", grant(ShareAccess::read()), "s4", None)
        .await
        .unwrap();
    let third = only_grant(store, &id).await.policy_revision;
    assert!(third > second, "a revision is never reused");
    assert_eq!(anew.policy_revision, Some(third));
    assert_eq!(
        change_from(
            store,
            &id,
            "phone",
            ShareGrantTransition::Revoke,
            "r1",
            None
        )
        .await
        .unwrap(),
        revoked
    );
    assert!(store_granted(store, &id, "phone").await);
}

/// Whether `receiver` holds a grant on `id`.
async fn store_granted(
    store: &LocalConversationStore,
    id: &ConversationId,
    receiver: &str,
) -> bool {
    use crate::conversation::application::ShareGrants;
    store.is_granted(id, receiver).await.unwrap()
}

/// Row J30: a share or revoke naming the policy revision the grant holds is
/// made, and the share answers the new revision; naming any other — the
/// grant changed or revoked since, or none held — is refused
/// `ShareGrantChanged`, and nothing is written: not the grant, the journal,
/// or the catalogue. Naming none makes the change whatever is held, as
/// before. A retry is answered before the comparison, so a change whose
/// answer was lost — a share's or a revoke's — is not refused by the
/// revision it moved itself.
#[tokio::test]
async fn a_change_naming_a_revision_the_grant_no_longer_holds_is_refused_and_writes_nothing() {
    use crate::conversation::application::{ConversationError, ShareGrantTransition};
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    let grant = |access: ShareAccess| ShareGrantTransition::Grant(access);
    let read = change_from(store, &id, "phone", grant(ShareAccess::read()), "s1", None)
        .await
        .unwrap()
        .policy_revision;
    // Built on what it read: made, and answers the revision it left.
    let driving = change_from(store, &id, "phone", grant(no_shell()), "s2", read)
        .await
        .unwrap();
    assert!(driving.applied);
    let held = only_grant(store, &id).await;
    assert_eq!(driving.policy_revision, Some(held.policy_revision));
    assert!(held.policy_revision > read.unwrap());
    // Another window's read, now stale: refused, whatever it asks.
    let journal = role_journal(&opened.path).len();
    let head = owner_head(store).await;
    for (transition, request) in [
        (grant(rules()), "stale-share"),
        (grant(no_shell()), "stale-same"),
        (ShareGrantTransition::Revoke, "stale-revoke"),
    ] {
        let refused = change_from(store, &id, "phone", transition, request, read).await;
        assert!(
            matches!(refused, Err(ConversationError::ShareGrantChanged)),
            "{request}: {:?}",
            refused.map_err(|error| error.to_string())
        );
        assert_eq!(only_grant(store, &id).await, held, "{request}");
    }
    assert_eq!(role_journal(&opened.path).len(), journal);
    assert_eq!(owner_head(store).await, head);
    // A retry of the change that moved the grant is answered, not refused.
    assert_eq!(
        change_from(store, &id, "phone", grant(no_shell()), "s2", read)
            .await
            .unwrap(),
        driving
    );
    // A revoke built on what is held is made.
    let revoked = change_from(
        store,
        &id,
        "phone",
        ShareGrantTransition::Revoke,
        "r1",
        driving.policy_revision,
    )
    .await
    .unwrap();
    assert!(revoked.applied);
    assert!(!store_granted(store, &id, "phone").await);
    // Its retry names the revision it took away, which no grant holds now:
    // answered from its journal row, not refused.
    assert_eq!(
        change_from(
            store,
            &id,
            "phone",
            ShareGrantTransition::Revoke,
            "r1",
            driving.policy_revision,
        )
        .await
        .unwrap(),
        revoked
    );
    // None held: any expected revision but 0 is stale.
    let journal = role_journal(&opened.path).len();
    for (transition, request) in [
        (grant(ShareAccess::read()), "gone-share"),
        (ShareGrantTransition::Revoke, "gone-revoke"),
    ] {
        let refused = change_from(
            store,
            &id,
            "phone",
            transition,
            request,
            driving.policy_revision,
        )
        .await;
        assert!(
            matches!(refused, Err(ConversationError::ShareGrantChanged)),
            "{request}: {:?}",
            refused.map_err(|error| error.to_string())
        );
    }
    assert!(!store_granted(store, &id, "phone").await);
    assert_eq!(role_journal(&opened.path).len(), journal);
    // Naming none: made whatever is held, as before.
    assert!(
        change_from(store, &id, "phone", grant(rules()), "s3", None)
            .await
            .unwrap()
            .applied
    );
    assert!(
        change_from(store, &id, "phone", grant(ShareAccess::read()), "s4", None)
            .await
            .unwrap()
            .applied
    );
    assert_eq!(only_grant(store, &id).await.access, ShareAccess::read());
}

/// Row J30: revision 0 names no grant held — a revision is a journal
/// sequence, which starts at 1. A first grant naming it is made; a second
/// first grant naming it, built on the same list, is refused
/// `ShareGrantChanged` and writes nothing, so neither window overwrites a
/// grant it never saw.
#[tokio::test]
async fn a_first_grant_naming_no_revision_is_refused_once_another_landed() {
    use crate::conversation::application::{ConversationError, ShareGrantTransition};
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    let first = change_from(
        store,
        &id,
        "phone",
        ShareGrantTransition::Grant(ShareAccess::read()),
        "first",
        Some(0),
    )
    .await
    .unwrap();
    assert!(first.applied);
    assert!(first.policy_revision.is_some_and(|revision| revision > 0));
    let held = only_grant(store, &id).await;
    let journal = role_journal(&opened.path).len();
    let head = owner_head(store).await;
    let second = change_from(
        store,
        &id,
        "phone",
        ShareGrantTransition::Grant(no_shell()),
        "second",
        Some(0),
    )
    .await;
    assert!(
        matches!(second, Err(ConversationError::ShareGrantChanged)),
        "{:?}",
        second.map_err(|error| error.to_string())
    );
    assert_eq!(only_grant(store, &id).await, held);
    assert_eq!(role_journal(&opened.path).len(), journal);
    assert_eq!(owner_head(store).await, head);
    // A revoke naming 0 meets a grant, so it is refused too.
    assert!(matches!(
        change_from(
            store,
            &id,
            "phone",
            ShareGrantTransition::Revoke,
            "gone",
            Some(0),
        )
        .await,
        Err(ConversationError::ShareGrantChanged)
    ));
    assert_eq!(only_grant(store, &id).await, held);
}

/// Row J8: a revoke ends a grant of any role, journaled with the role and
/// policy it took away.
#[tokio::test]
async fn a_revoke_ends_a_grant_of_any_role() {
    use crate::conversation::application::ShareGrants;
    let comment = ShareAccess::new(Capabilities::comment(), None).unwrap();
    for (access, capabilities, policy) in [
        (comment, "read+comment", None),
        (
            no_shell(),
            "read+comment+drive",
            Some(r#"{"preset":"no_shell"}"#.to_owned()),
        ),
    ] {
        let opened = opened();
        let id = new_id();
        opened.store.create(owned(&id)).await.unwrap();
        assert!(share_as(&opened.store, &id, "phone", access, "share")
            .await
            .unwrap());
        assert!(revoke(&opened.store, &id, "phone").await);
        assert!(!opened.store.is_granted(&id, "phone").await.unwrap());
        assert!(opened.store.grants(&id).await.unwrap().grants.is_empty());
        assert!(device_pass(&opened.store, device("phone"), 0)
            .await
            .is_empty());
        let journal = role_journal(&opened.path);
        let last = journal.last().unwrap();
        assert_eq!(
            (last.0.as_str(), last.1.as_str(), &last.2, &last.3),
            (capabilities, "none", &policy, &None)
        );
    }
}

/// Row J9: a conversation holds at most `MAX_DRIVE_GRANTS_PER_CONVERSATION`
/// Drive grants; one more, new or a change from another role, is refused and
/// writes nothing.
#[tokio::test]
async fn a_conversation_holds_a_bounded_number_of_drive_grants() {
    use crate::conversation::application::{
        ConversationError, ShareGrants, MAX_DRIVE_GRANTS_PER_CONVERSATION,
    };
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    for n in 0..MAX_DRIVE_GRANTS_PER_CONVERSATION {
        assert!(share_as(
            store,
            &id,
            &format!("driver-{n}"),
            no_shell(),
            &format!("drive-{n}")
        )
        .await
        .unwrap());
    }
    let journaled = role_journal(&opened.path).len();
    assert!(matches!(
        share_as(store, &id, "one-more", no_shell(), "one-more").await,
        Err(ConversationError::InvalidInput)
    ));
    assert!(!store.is_granted(&id, "one-more").await.unwrap());
    let before_reader = store.grants(&id).await.unwrap().change_revision;
    assert!(
        share_as(store, &id, "reader", ShareAccess::read(), "reader")
            .await
            .unwrap()
    );
    assert!(
        store.grants(&id).await.unwrap().change_revision > before_reader,
        "a new grant moves the revision a shares page is bound to"
    );
    assert!(matches!(
        share_as(store, &id, "reader", no_shell(), "reader-drives").await,
        Err(ConversationError::InvalidInput)
    ));
    assert_eq!(role_journal(&opened.path).len(), journaled + 1);
    let grants = store.grants(&id).await.unwrap().grants;
    let driving = grants
        .iter()
        .filter(|grant| grant.access.capabilities() == Capabilities::drive())
        .count();
    assert_eq!(driving as i64, MAX_DRIVE_GRANTS_PER_CONVERSATION);
    // A driver's own policy can still change: it takes no new Drive grant,
    // and the revision a shares page is bound to stays.
    let before_policy = store.grants(&id).await.unwrap().change_revision;
    assert!(share_as(store, &id, "driver-0", rules(), "driver-0-rules")
        .await
        .unwrap());
    assert_eq!(
        store.grants(&id).await.unwrap().change_revision,
        before_policy
    );
}

/// A share change is admitted by the access policy inside its transaction:
/// a store given none, or a policy that cannot decide, refuses every change
/// as storage trouble and writes nothing; a policy that refuses ownership
/// is told as a conversation not found, and a grant on a deleted one as
/// deleted, while its owner may still revoke there.
#[tokio::test]
async fn a_share_change_is_admitted_by_the_access_policy_in_its_transaction() {
    use crate::conversation::application::{
        AccessAsk, AccessDecision, AccessDenial, ConversationAccessPolicyEvaluator,
        ShareGrantTransition, ShareGrants,
    };
    struct Undecided;
    impl ConversationAccessPolicyEvaluator for Undecided {
        fn evaluate(&self, _: &AccessAsk<'_>) -> AccessDecision {
            AccessDecision::Deny(AccessDenial::Unavailable)
        }
    }
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let change = |transition| {
        grant_change(
            transition,
            &id,
            Some("phone"),
            "phone-credential",
            &uuid::Uuid::new_v4().to_string(),
        )
    };
    for store in [
        LocalConversationStore::open(&opened.path).unwrap(),
        LocalConversationStore::open(&opened.path)
            .unwrap()
            .with_access_policy(Arc::new(Undecided)),
    ] {
        assert!(matches!(
            store
                .change(change(ShareGrantTransition::Grant(ShareAccess::read())))
                .await,
            Err(ConversationError::Metadata)
        ));
        assert!(matches!(
            store.change(change(ShareGrantTransition::Revoke)).await,
            Err(ConversationError::Metadata)
        ));
    }
    assert!(!opened.store.is_granted(&id, "phone").await.unwrap());
    let mut stranger = change(ShareGrantTransition::Grant(ShareAccess::read()));
    stranger.initiator.principal_id = PrincipalId::new("mallory").unwrap();
    assert!(matches!(
        opened.store.change(stranger).await,
        Err(ConversationError::NotFound)
    ));
    assert!(grant(&opened.store, &id, "phone").await);
    opened
        .store
        .record_deletion(&id, deletion("delete-1"))
        .await
        .unwrap();
    assert!(matches!(
        opened
            .store
            .change(change(ShareGrantTransition::Grant(ShareAccess::read())))
            .await,
        Err(ConversationError::Deleted)
    ));
    assert!(revoke(&opened.store, &id, "phone").await);
}

/// ADR 803 step 4: the command hosts a Drive grant's policy names are kept
/// with it and read back at its revision; changing only the hosts is a new
/// revision, and the earlier one still names the hosts it held.
#[tokio::test]
async fn a_revisions_command_hosts_are_read_from_its_journal_entry() {
    use crate::conversation::application::ShareGrants;
    use crate::conversation::domain::CommandHosts;
    let on = |hosts: &[&str]| {
        drive_with(ToolPolicy::preset(ToolPreset::All).with_command_hosts(
            CommandHosts::new(hosts.iter().map(|host| (*host).to_owned()).collect()).unwrap(),
        ))
    };
    let peer = format!("peer:{}", "ab".repeat(32));
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    let both = on(&["devbox", &peer]);
    assert!(share_as(store, &id, "phone", both.clone(), "both")
        .await
        .unwrap());
    let first = only_grant(store, &id).await;
    assert_eq!(first.access, both);
    let one = on(&["devbox"]);
    assert!(share_as(store, &id, "phone", one.clone(), "one")
        .await
        .unwrap());
    let second = only_grant(store, &id).await.policy_revision;
    assert_ne!(second, first.policy_revision);
    let at = |revision: u64| {
        let id = &id;
        async move { store.policy_at(id, "phone", revision).await.unwrap() }
    };
    let held = at(first.policy_revision).await.unwrap();
    assert_eq!(
        held.command_hosts().names().iter().collect::<Vec<_>>(),
        ["devbox", &peer]
    );
    assert_eq!(Some(&held), both.policy());
    assert_eq!(at(second).await.as_ref(), one.policy());
}

/// Row J26: the policy a Drive grant held at a revision is read from that
/// revision's journal entry, so a later change does not change it. A
/// revision that is no Drive grant of that party on that conversation, or
/// none at all, names no policy.
#[tokio::test]
async fn a_revisions_policy_is_read_from_its_journal_entry() {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    let opened = opened();
    let store = &opened.store;
    let (id, other) = (new_id(), new_id());
    store.create(owned(&id)).await.unwrap();
    store.create(owned(&other)).await.unwrap();
    assert!(share_as(store, &id, "phone", ShareAccess::read(), "read")
        .await
        .unwrap());
    let read = only_grant(store, &id).await.policy_revision;
    assert!(share_as(store, &id, "phone", no_shell(), "no-shell")
        .await
        .unwrap());
    let shell_less = only_grant(store, &id).await.policy_revision;
    assert!(share_as(store, &id, "phone", rules(), "rules")
        .await
        .unwrap());
    let ruled = only_grant(store, &id).await.policy_revision;
    let at = |id: ConversationId, receiver: &'static str, revision: u64| async move {
        store.policy_at(&id, receiver, revision).await.unwrap()
    };
    assert_eq!(
        at(id.clone(), "phone", shell_less).await,
        no_shell().policy().cloned()
    );
    assert_eq!(
        at(id.clone(), "phone", ruled).await,
        rules().policy().cloned()
    );
    assert_eq!(at(id.clone(), "phone", read).await, None);
    assert_eq!(at(id.clone(), "laptop", ruled).await, None);
    assert_eq!(at(other.clone(), "phone", ruled).await, None);
    assert_eq!(at(id.clone(), "phone", ruled + 100).await, None);
    assert_eq!(at(id.clone(), "phone", u64::MAX).await, None);
    assert!(
        store
            .change(grant_change(
                ShareGrantTransition::Revoke,
                &id,
                Some("phone"),
                "phone-credential",
                "revoke",
            ))
            .await
            .unwrap()
            .applied
    );
    assert_eq!(
        at(id.clone(), "phone", shell_less).await,
        no_shell().policy().cloned()
    );
}

/// Row J31: a Drive turn's save keeps the admitted revision, and a grant
/// change waits until that hold is dropped. A missing grant, another
/// revision, or a revision cell that is not a journal sequence is `revoked`.
#[tokio::test]
async fn a_turn_is_saved_only_while_its_grant_stays_at_the_admitted_revision() {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    use tokio::sync::Notify;
    let opened = opened();
    let Opened {
        _directory,
        path,
        store,
    } = opened;
    let store = Arc::new(store);
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    assert!(share_as(&store, &id, "phone", no_shell(), "share")
        .await
        .unwrap());
    let revision = only_grant(&store, &id).await.policy_revision;
    assert!(matches!(
        ShareGrants::hold_revision(&*store, &id, "other", revision).await,
        Err(ConversationError::Revoked)
    ));
    assert!(matches!(
        ShareGrants::hold_revision(&*store, &id, "phone", revision + 1).await,
        Err(ConversationError::Revoked)
    ));

    let hold = ShareGrants::hold_revision(&*store, &id, "phone", revision)
        .await
        .unwrap();
    let started = Arc::new(Notify::new());
    let revoking = Arc::clone(&store);
    let revoke_id = id.clone();
    let started_in_task = Arc::clone(&started);
    let mut revoke = tokio::spawn(async move {
        started_in_task.notify_one();
        revoking
            .change(grant_change(
                ShareGrantTransition::Revoke,
                &revoke_id,
                Some("phone"),
                "phone-credential",
                "revoke-during-hold",
            ))
            .await
    });
    started.notified().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut revoke)
            .await
            .is_err(),
        "a revoke committed while the revision was held"
    );
    // Another conversation's grant is not this permit. Its revoke finishes
    // while the first conversation's revision is still held.
    let other = new_id();
    store.create(owned(&other)).await.unwrap();
    assert!(share_as(&store, &other, "phone", no_shell(), "other-share")
        .await
        .unwrap());
    let other_store = Arc::clone(&store);
    let other_id = other.clone();
    let other_revoke = tokio::spawn(async move {
        other_store
            .change(grant_change(
                ShareGrantTransition::Revoke,
                &other_id,
                Some("phone"),
                "phone-credential",
                "revoke-other",
            ))
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_secs(2), other_revoke)
            .await
            .expect("another conversation's revoke is not blocked by this hold")
            .expect("the other revoke task")
            .unwrap()
            .applied
    );
    drop(hold);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), &mut revoke)
            .await
            .expect("the revoke finishes")
            .expect("the revoke task")
            .unwrap()
            .applied
    );
    assert!(matches!(
        ShareGrants::hold_revision(&*store, &id, "phone", revision).await,
        Err(ConversationError::Revoked)
    ));

    let again = new_id();
    store.create(owned(&again)).await.unwrap();
    assert!(share_as(&store, &again, "phone", no_shell(), "share-again")
        .await
        .unwrap());
    let live = only_grant(&store, &again).await.policy_revision;
    let connection = raw(&path);
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .unwrap();
    connection
        .execute(
            "UPDATE share_grants SET policy_revision = 0
             WHERE resource_kind = 'conversation' AND resource_id = ?1",
            params![again.to_string()],
        )
        .unwrap();
    assert!(matches!(
        ShareGrants::hold_revision(&*store, &again, "phone", live).await,
        Err(ConversationError::Revoked)
    ));
}

/// Releases a paused grant write when the test future is dropped.
struct ReleaseOnDrop(Arc<super::store::GrantWritePause>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Row J31: cancelling a grant change while its write is in flight does not
/// drop the permit. A hold of the revision that write is about to replace
/// waits until the write finishes, then sees the grant gone.
#[tokio::test]
async fn a_cancelled_grant_change_keeps_its_permit_until_the_write_finishes() {
    use crate::conversation::application::{ShareGrantTransition, ShareGrants};
    let opened = opened();
    let store = Arc::new(opened.store);
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    assert!(share_as(&store, &id, "phone", no_shell(), "share")
        .await
        .unwrap());
    let revision = only_grant(&store, &id).await.policy_revision;
    let mut watch = store.watch_reader(&org(), &alice(), "phone").unwrap();
    let pause = super::store::GrantWritePause::new();
    // A failed assertion still lets the blocked write finish, so the runtime
    // is not left waiting on that task.
    let _release = ReleaseOnDrop(Arc::clone(&pause));
    store.pause_grant_writes(Arc::clone(&pause));
    let revoking = Arc::clone(&store);
    let revoke_id = id.clone();
    let change = tokio::spawn(async move {
        revoking
            .change(grant_change(
                ShareGrantTransition::Revoke,
                &revoke_id,
                Some("phone"),
                "phone-credential",
                "revoke-cancelled",
            ))
            .await
    });
    let entered = {
        let pause = Arc::clone(&pause);
        tokio::task::spawn_blocking(move || pause.wait_until_entered())
    };
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the write entered")
        .expect("the write's wait");
    change.abort();
    assert!(change
        .await
        .expect_err("the change task was cancelled")
        .is_cancelled());
    let mut holding = {
        let store = Arc::clone(&store);
        let id = id.clone();
        tokio::spawn(
            async move { ShareGrants::hold_revision(&*store, &id, "phone", revision).await },
        )
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut holding)
            .await
            .is_err(),
        "hold_revision read the revision while the cancelled change's write still held the gate"
    );
    pause.release();
    let held = tokio::time::timeout(Duration::from_secs(5), holding)
        .await
        .expect("the hold finishes once the write does")
        .expect("the hold task");
    assert!(
        matches!(held, Err(ConversationError::Revoked)),
        "the hold kept a revision the cancelled write replaced"
    );
    assert!(
        ShareGrants::grants(&*store, &id)
            .await
            .unwrap()
            .grants
            .is_empty(),
        "the detached write did not revoke the grant"
    );
    assert_eq!(
        catalogue_watch_ready(&mut watch),
        CatalogueWatchState::Dirty
    );
}

/// #724: head and notices follow the same receiver, including an empty
/// catalogue after the last revoke. Unrelated owners and receivers stay quiet.
#[tokio::test]
async fn paired_catalogue_notices_follow_only_the_readers_head() {
    let opened = opened();
    let store = &opened.store;
    let mut owner = store.watch(&org(), &alice()).unwrap();
    let mut phone = store.watch_reader(&org(), &alice(), "phone").unwrap();
    let mut tablet = store.watch_reader(&org(), &alice(), "tablet").unwrap();
    let mut wrong_owner = store
        .watch_reader(&org(), &PrincipalId::new("wrong-owner").unwrap(), "phone")
        .unwrap();
    let mut wrong_org = store
        .watch_reader(
            &OrganizationId::new("wrong-org").unwrap(),
            &alice(),
            "phone",
        )
        .unwrap();
    let (first, second) = (new_id(), new_id());
    store.create(owned(&first)).await.unwrap();
    store.create(owned(&second)).await.unwrap();
    assert_eq!(
        catalogue_watch_ready(&mut owner),
        CatalogueWatchState::Dirty
    );
    catalogue_watch_pending(&mut phone);
    catalogue_watch_pending(&mut tablet);
    assert!(grant(store, &first, "phone").await);
    assert_eq!(
        catalogue_watch_ready(&mut phone),
        CatalogueWatchState::Dirty
    );
    let before = store
        .head(&org(), &alice(), &device("phone"))
        .await
        .unwrap();
    for n in 0..100 {
        store
            .record(&second, said("ungranted", n + 10))
            .await
            .unwrap();
    }
    assert!(grant(store, &second, "tablet").await);
    assert_eq!(
        store
            .head(&org(), &alice(), &device("phone"))
            .await
            .unwrap(),
        before
    );
    catalogue_watch_pending(&mut phone);
    assert_eq!(
        catalogue_watch_ready(&mut tablet),
        CatalogueWatchState::Dirty
    );
    store.record(&first, said("granted", 200)).await.unwrap();
    assert_eq!(
        catalogue_watch_ready(&mut phone),
        CatalogueWatchState::Dirty
    );
    assert!(revoke(store, &first, "phone").await);
    assert_eq!(
        catalogue_watch_ready(&mut phone),
        CatalogueWatchState::Dirty
    );
    let revoked = store
        .head(&org(), &alice(), &device("phone"))
        .await
        .unwrap();
    assert!(revoked.revision > before.revision);
    assert!(device_pass(store, device("phone"), before.revision)
        .await
        .is_empty());
    store
        .record(&first, said("after revoke", 201))
        .await
        .unwrap();
    catalogue_watch_pending(&mut phone);
    catalogue_watch_pending(&mut wrong_owner);
    catalogue_watch_pending(&mut wrong_org);
}

/// Characterizes the remaining #724 disclosure: an old receiver's head still
/// advances on later replacements. Preserve cursor agreement until a durable
/// floor permits narrowing that head without regressing saved boundaries.
#[tokio::test]
async fn historical_receiver_replacements_notify_exactly_when_their_head_moves() {
    let opened = opened();
    let store = &opened.store;
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    let mut first = store.watch_reader(&org(), &alice(), "first").unwrap();
    let mut second = store.watch_reader(&org(), &alice(), "second").unwrap();
    let mut third = store.watch_reader(&org(), &alice(), "third").unwrap();
    for receiver in ["first", "second", "third"] {
        let mut change = grant_change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            &id,
            Some(receiver),
            &format!("credential-{receiver}"),
            &format!("request-{receiver}"),
        );
        change.principal_id = Some(PrincipalId::new("one-grantee").unwrap());
        assert!(store.change(change).await.unwrap().applied);
        assert_eq!(
            catalogue_watch_ready(&mut first),
            CatalogueWatchState::Dirty
        );
        if receiver != "first" {
            assert_eq!(
                catalogue_watch_ready(&mut second),
                CatalogueWatchState::Dirty
            );
        } else {
            catalogue_watch_pending(&mut second);
        }
        if receiver == "third" {
            assert_eq!(
                catalogue_watch_ready(&mut third),
                CatalogueWatchState::Dirty
            );
        } else {
            catalogue_watch_pending(&mut third);
        }
    }
    store
        .record(&id, said("only third still holds the row", 100))
        .await
        .unwrap();
    catalogue_watch_pending(&mut first);
    catalogue_watch_pending(&mut second);
    assert_eq!(
        catalogue_watch_ready(&mut third),
        CatalogueWatchState::Dirty
    );
}

struct DatabaseUnlockedOnNotice {
    store: Arc<LocalConversationStore>,
    observed: AtomicBool,
}
impl Wake for DatabaseUnlockedOnNotice {
    fn wake(self: Arc<Self>) {
        self.observed
            .store(self.store.mutation_is_unlocked(), Ordering::SeqCst);
    }
}

#[tokio::test]
async fn catalogue_publication_releases_database_before_invoking_a_waker() {
    let opened = opened();
    let store = Arc::new(opened.store);
    let mut watch = store.watch(&org(), &alice()).unwrap();
    let observer = Arc::new(DatabaseUnlockedOnNotice {
        store: store.clone(),
        observed: AtomicBool::new(false),
    });
    let waker = Waker::from(observer.clone());
    let mut wait = Box::pin(watch.changed());
    assert!(wait
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    store.create(owned(&new_id())).await.unwrap();
    assert!(observer.observed.load(Ordering::SeqCst));
    assert_eq!(wait.await, CatalogueWatchState::Dirty);
}

#[tokio::test]
async fn a_failed_scoped_head_capture_does_not_broaden_a_committed_notice() {
    let opened = opened();
    let store = &opened.store;
    let mut owner = store.watch(&org(), &alice()).unwrap();
    let mut phone = store.watch_reader(&org(), &alice(), "phone").unwrap();
    // Only this synthetic database is damaged. Owner writes still work, while
    // a scoped head cannot query the grant journal.
    raw(&opened.path)
        .execute(
            "ALTER TABLE share_grant_changes RENAME TO missing_grant_journal",
            [],
        )
        .unwrap();
    assert!(store
        .head(&org(), &alice(), &device("phone"))
        .await
        .is_err());
    let id = new_id();
    store.create(owned(&id)).await.unwrap();
    assert!(ConversationRepository::load(store, &id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        catalogue_watch_ready(&mut owner),
        CatalogueWatchState::Dirty
    );
    catalogue_watch_pending(&mut phone);
}
