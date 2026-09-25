//! Ownership, tombstones and summaries in one database: create-once
//! ownership, tombstones and summaries that cannot stand without their record,
//! rows refused rather than repaired, and a list that reads only its owner's.
use super::store::{LocalConversationStore, LIST, UNFINISHED};
use crate::agents::domain::AgentId;
use crate::conversation::{
    application::{
        ConversationCreationDisposition, ConversationError, ConversationListing,
        ConversationRepository, ConversationSummaries,
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
    )
    .unwrap()
}
fn owned(id: &ConversationId) -> Conversation {
    owned_by(id, "org", "alice", 1)
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

/// Rows of `count` conversations owned by `owner`, each with a summary,
/// written in one transaction.
fn many(path: &Path, owner: &str, count: usize) {
    let mut raw = raw(path);
    let transaction = raw.transaction().unwrap();
    for n in 0..count {
        let id = new_id().to_string();
        transaction
            .execute(
                "INSERT INTO conversations VALUES (?1, 'org', ?2, 'panel', 'create', 1, 'claude')",
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
        "INSERT INTO conversations VALUES (?1, 'org', 'alice', 'panel', 'create', 1, 'claude')",
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
