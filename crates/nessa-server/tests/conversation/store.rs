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
        ConversationSummaries, Reader, WatchCatalogue,
    },
    domain::{Conversation, ConversationDeletion, ProviderSessionErasure, ProviderSessionLink},
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
    sync::Arc,
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
    let store = LocalConversationStore::open(&path).unwrap();
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
        opened.store.head(&org(), &alice()).await.unwrap().revision,
        2
    );
    drop(opened.store);
    let reopened = LocalConversationStore::open(&opened.path).unwrap();
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
    assert_eq!(reopened.head(&org(), &alice()).await.unwrap().revision, 2);
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
            expected: 4
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
            expected: 4
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
    let store = LocalConversationStore::open(&path).unwrap();
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
    let store = LocalConversationStore::open(&path).unwrap();
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
    let store = LocalConversationStore::open(&path).unwrap();
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
        let head = opened.store.head(&org(), &alice()).await.unwrap();
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
        let reopened = LocalConversationStore::open(&opened.path).unwrap();
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
        .head(&org(), &alice())
        .await
        .unwrap()
        .incarnation;
    assert_eq!(
        opened.store.head(&org(), &alice()).await.unwrap().revision,
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
        opened.store.head(&org(), &alice()).await.unwrap().revision,
        4
    );
    drop(opened.store);
    let reopened = LocalConversationStore::open(&opened.path).unwrap();
    assert_eq!(
        reopened.head(&org(), &alice()).await.unwrap().incarnation,
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
        .head(&org(), &alice())
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
    assert_eq!(opened.store.head(&org(), &bob).await.unwrap().revision, 20);
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
    let boundary = opened.store.head(&org(), &alice()).await.unwrap().revision;
    let writer = LocalConversationStore::open(&opened.path).unwrap();
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
        .head(&org(), &alice())
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
        .head(&org(), &alice())
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
        .head(&org(), &alice())
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
        opened.store.head(&org(), &alice()).await.unwrap().revision,
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
        opened.store.head(&org(), &alice()).await.unwrap().revision,
        2
    );
    raw.execute_batch("CREATE TRIGGER refuse_deletion BEFORE INSERT ON deletions BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
    assert!(matches!(
        opened.store.record_deletion(&id, deletion("refused")).await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(
        opened.store.head(&org(), &alice()).await.unwrap().revision,
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
        opened.store.head(&org(), &alice()).await.unwrap().revision,
        3
    );
}

#[tokio::test]
async fn a_failed_creation_does_not_publish_an_owner_or_spend_a_revision() {
    let opened = opened();
    let id = new_id();
    let incarnation = opened
        .store
        .head(&org(), &alice())
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
        opened.store.head(&org(), &alice()).await.unwrap().revision,
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
        .head(&org(), &alice())
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
        opened.store.head(&org(), &alice()).await,
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
    assert_eq!(opened.store.head(&org(), &bob).await.unwrap().revision, 1);
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
        opened.store.head(&org(), &alice()).await,
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
    let head = opened.store.head(&org(), &alice()).await.unwrap();
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
    let head = opened.store.head(&org(), &alice()).await.unwrap();
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
    assert!(opened.store.head(&org(), &alice()).await.is_err());
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
    let head = opened.store.head(&organization, &owner).await.unwrap();
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
        let head = opened.store.head(&org(), &alice()).await.unwrap();
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
    let head = opened.store.head(&org(), &alice()).await.unwrap();
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
        opened.store.head(&org(), &alice()).await.unwrap().revision,
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
        opened.store.head(&org(), &alice()).await.unwrap().revision,
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
        opened.store.head(&org(), &alice()).await.unwrap().revision,
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
    transition: crate::conversation::application::ReadGrantTransition,
    id: &ConversationId,
    receiver: Option<&str>,
    credential: &str,
    request: &str,
) -> crate::conversation::application::ReadGrantChange {
    crate::conversation::application::ReadGrantChange {
        transition,
        conversation_id: id.clone(),
        receiver_id: receiver.map(str::to_owned),
        credential_id: nessa_auth::domain::CredentialId::new(credential).unwrap(),
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
    use crate::conversation::application::{ReadGrantTransition, ReadGrants};
    store
        .change(grant_change(
            ReadGrantTransition::Grant,
            id,
            Some(receiver),
            &format!("{receiver}-credential"),
            &uuid::Uuid::new_v4().to_string(),
        ))
        .await
        .unwrap()
}
async fn revoke(store: &LocalConversationStore, id: &ConversationId, receiver: &str) -> bool {
    use crate::conversation::application::{ReadGrantTransition, ReadGrants};
    store
        .change(grant_change(
            ReadGrantTransition::Revoke,
            id,
            None,
            &format!("{receiver}-credential"),
            &uuid::Uuid::new_v4().to_string(),
        ))
        .await
        .unwrap()
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
    let head = store.head(&org(), &alice()).await.unwrap();
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
    let head = opened.store.head(&org(), &alice()).await.unwrap();
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
    let completed = opened.store.head(&org(), &alice()).await.unwrap().revision;
    assert!(grant(&opened.store, &old, "phone").await);
    let head = opened.store.head(&org(), &alice()).await.unwrap().revision;
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
    let before = opened.store.head(&org(), &alice()).await.unwrap().revision;
    assert!(revoke(&opened.store, &id, "phone").await);
    let head = opened.store.head(&org(), &alice()).await.unwrap();
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
    use crate::conversation::application::ReadGrants;
    assert!(!opened.store.is_granted(&id, "phone").await.unwrap());
}

/// Row G7.
#[tokio::test]
async fn a_grant_names_one_conversation_and_nothing_else() {
    use crate::conversation::application::ReadGrants;
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
    let head = opened.store.head(&org(), &alice()).await.unwrap().revision;
    assert!(!grant(&opened.store, &id, "phone").await);
    assert_eq!(
        opened.store.head(&org(), &alice()).await.unwrap().revision,
        head
    );
    let unchanged: i64 = raw(&opened.path)
        .query_row(
            "SELECT COUNT(*) FROM read_grant_changes WHERE before = after",
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
    use crate::conversation::application::{ReadGrantTransition, ReadGrants};
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    assert!(grant(&opened.store, &id, "phone").await);
    let share = || {
        grant_change(
            ReadGrantTransition::Grant,
            &id,
            Some("phone"),
            "phone-credential",
            "share-again",
        )
    };
    assert!(!opened.store.change(share()).await.unwrap());
    assert!(revoke(&opened.store, &id, "phone").await);
    let head = opened.store.head(&org(), &alice()).await.unwrap().revision;
    assert!(!opened.store.change(share()).await.unwrap());
    assert!(!opened.store.is_granted(&id, "phone").await.unwrap());
    assert_eq!(
        opened.store.head(&org(), &alice()).await.unwrap().revision,
        head
    );
}

/// Row G16: a retried request answers what it answered the first time. A
/// share whose reply was lost, retried after a later unshare, does not grant
/// again; the same request naming another device or change is refused.
#[tokio::test]
async fn a_retried_share_replays_its_answer_and_never_undoes_a_later_unshare() {
    use crate::conversation::application::{ConversationError, ReadGrantTransition, ReadGrants};
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let share = || {
        grant_change(
            ReadGrantTransition::Grant,
            &id,
            Some("phone"),
            "phone-credential",
            "share-1",
        )
    };
    assert!(opened.store.change(share()).await.unwrap());
    assert!(revoke(&opened.store, &id, "phone").await);
    assert!(opened.store.change(share()).await.unwrap());
    assert!(!opened.store.is_granted(&id, "phone").await.unwrap());
    for (transition, credential) in [
        (ReadGrantTransition::Revoke, "phone-credential"),
        (ReadGrantTransition::Grant, "tablet-credential"),
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
        .query_row("SELECT COUNT(*) FROM read_grant_changes", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(journaled, 2);
}

/// Row G17: a conversation holds at most
/// `MAX_READ_GRANTS_PER_CONVERSATION` grants, so its shares answer fits one
/// frame; one more is refused and writes nothing.
#[tokio::test]
async fn a_conversation_holds_a_bounded_number_of_grants() {
    use crate::conversation::application::{
        ConversationError, ReadGrantTransition, ReadGrants, MAX_READ_GRANTS_PER_CONVERSATION,
    };
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    for device in 0..MAX_READ_GRANTS_PER_CONVERSATION {
        assert!(grant(&opened.store, &id, &format!("device-{device}")).await);
    }
    let refused = opened
        .store
        .change(grant_change(
            ReadGrantTransition::Grant,
            &id,
            Some("one-more"),
            "one-more-credential",
            "one-more",
        ))
        .await;
    assert!(matches!(refused, Err(ConversationError::InvalidInput)));
    assert!(!opened.store.is_granted(&id, "one-more").await.unwrap());
    assert_eq!(
        opened.store.grants(&id).await.unwrap().len() as i64,
        MAX_READ_GRANTS_PER_CONVERSATION
    );
}

/// Row G8, in the store: a grant or revoke on another owner's conversation
/// is refused as not found inside the transaction that would write it, and
/// writes nothing.
#[tokio::test]
async fn a_grant_on_another_owners_conversation_writes_nothing() {
    use crate::conversation::application::{ConversationError, ReadGrantTransition, ReadGrants};
    let opened = opened();
    let id = new_id();
    opened
        .store
        .create(owned_by(&id, "org", "bob", 1))
        .await
        .unwrap();
    for transition in [ReadGrantTransition::Grant, ReadGrantTransition::Revoke] {
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
            "SELECT (SELECT COUNT(*) FROM read_grants) + (SELECT COUNT(*) FROM read_grant_changes)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(written, 0);
}

/// One `read_grant_changes` row, in column order.
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
        use crate::conversation::application::{ReadGrantTransition, ReadGrants};
        for (transition, receiver, request) in [
            (ReadGrantTransition::Grant, Some("phone"), "share"),
            (ReadGrantTransition::Revoke, None, "unshare"),
        ] {
            assert!(opened
                .store
                .change(grant_change(
                    transition,
                    &id,
                    receiver,
                    "phone-credential",
                    request
                ))
                .await
                .unwrap());
        }
    }
    let head = opened.store.head(&org(), &alice()).await.unwrap().revision;
    let connection = raw(&opened.path);
    let mut statement = connection
        .prepare(
            "SELECT conversation_id, receiver_id, credential_id, before, after, initiator,
                    surface, request, changed_at_ms, revision
             FROM read_grant_changes ORDER BY sequence",
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
