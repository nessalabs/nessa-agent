//! Ownership, tombstones and summaries in one database: create-once
//! ownership, tombstones and summaries that cannot stand without their record,
//! rows refused rather than repaired, and a list that reads only its owner's.
use super::store::{LocalConversationStore, LIST, UNFINISHED};
use crate::agents::domain::AgentId;
use crate::conversation::{
    application::{
        CatalogueKey, CataloguePageRequest, ConversationCatalogue, ConversationCreationDisposition,
        ConversationError, ConversationListing, ConversationModeApplication,
        ConversationModeRequest, ConversationModeRequestState, ConversationRepository,
        ConversationSummaries,
    },
    domain::{
        Conversation, ConversationDeletion, ConversationId, ConversationSummary,
        ProviderSessionErasure, ProviderSessionLink,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{params, Connection, StatementStatus};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use std::path::{Path, PathBuf};
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
        crate::conversation::domain::ConversationModelId::new("test-model").unwrap(),
        crate::conversation::domain::ConversationApprovalMode::Ask,
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
        prior: crate::conversation::domain::ConversationApprovalMode::Ask,
        requested: crate::conversation::domain::ConversationApprovalMode::Auto,
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
        crate::conversation::domain::ConversationApprovalMode::Auto
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
            expected: 3
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
            expected: 3
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
    CataloguePageRequest {
        organization: org(),
        owner,
        incarnation: incarnation.into(),
        completed,
        boundary,
        after,
        limit,
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
        crate::conversation::domain::ConversationModelId::new("test-model").unwrap(),
        crate::conversation::domain::ConversationApprovalMode::Ask,
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
        crate::conversation::domain::ConversationModelId::new("test-model").unwrap(),
        crate::conversation::domain::ConversationApprovalMode::Ask,
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
    for n in 0..count {
        let id = new_id().to_string();
        transaction
            .execute(
                "INSERT INTO conversations VALUES (?1, 'org', ?2, 'panel', 'create', 1, 'claude', 'test-model', 'ask', 1, 1)",
                params![id, owner],
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
    let mut statement = raw.prepare(LIST).unwrap();
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
        .resolve(&org(), &alice(), &incarnation, &id)
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
        .resolve(&org(), &bob, &incarnation, &id)
        .await
        .unwrap()
        .is_none());
    opened.store.record(&id, said("hello", 2)).await.unwrap();
    let second = opened
        .store
        .resolve(&org(), &alice(), &incarnation, &id)
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
        .resolve(&org(), &alice(), &incarnation, &id)
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
        .resolve(&org(), &alice(), &incarnation, &id)
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
            .resolve(&org(), &alice(), &incarnation, &id)
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
        .resolve(&org(), &alice(), &incarnation, &id)
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
        .resolve(&org(), &other, &incarnation, &id)
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
        .resolve(&org(), &alice(), &incarnation, &id)
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
            .resolve(&org(), &alice(), &incarnation, &id)
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
        .resolve(&org(), &alice(), &incarnation, &id)
        .await
        .unwrap()
        .is_none());
    raw.execute_batch("DROP TRIGGER refuse_creation;").unwrap();
    opened.store.create(owned(&id)).await.unwrap();
    let current = opened
        .store
        .resolve(&org(), &alice(), &incarnation, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (current.descriptor.key.creation, current.descriptor.revision),
        (1, 1)
    );
}
