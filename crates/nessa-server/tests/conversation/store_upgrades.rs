//! A metadata file an older build wrote is brought forward at open and keeps
//! its rows: version 6 (before `unfinished_turns`), version 7 with or
//! without `model_requests`, which #975 added without raising the version,
//! and version 8 (before `agent_switches`).
//! `metadata_v6.sql` is version 6's definition exactly as that build shipped
//! it (docs/adr/todo/202-versioned-local-datasets.md, "Migrations").
use super::store::LocalConversationStore;
use crate::conversation::application::{
    ConversationListing, ConversationRepository, Reader, UnfinishedTurns,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{params, Connection};
use nessa_local_database::Schema;
use nessa_protocol::conversation::domain::ConversationId;
use std::path::{Path, PathBuf};

const VERSION_6: &str = include_str!("metadata_v6.sql");

/// The step #974 shipped, which a version 6 file runs to become the version
/// 7 file that build wrote, before #975 added `model_requests`.
const SIX_TO_SEVEN: &str =
    include_str!("../../src/conversation/infrastructure/schema_upgrades/6.sql");

const CONVERSATION: &str = "9a3a1f3e-2f0c-4a43-9a1c-6f1d7c2b8e10";

fn private_path() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    (directory, path)
}

/// A file at an older version, holding one conversation the way that build
/// wrote it: its owner's head, the ownership row, a summary and a settled
/// approval-mode request.
fn older_file(path: &Path, seven: bool) {
    let connection = nessa_local_database::open(path, &Schema::new(VERSION_6).unwrap()).unwrap();
    connection
        .execute_batch(&format!(
            "INSERT INTO catalogue_owners (organization, owner, head) VALUES ('org', 'alice', 1);
             INSERT INTO conversations (id, organization, owner, creator_surface, creation_action,
                 creation_requested_at_ms, agent, model, approval_mode, creation_revision,
                 change_revision)
             VALUES ('{CONVERSATION}', 'org', 'alice', 'panel', 'create', 1000, 'claude',
                 'test-model', 'ask', 1, 1);
             INSERT INTO summaries (conversation_id, title, preview, updated_at_ms, archived)
             VALUES ('{CONVERSATION}', 'Kept across the upgrade', 'hello', 2000, 0);
             INSERT INTO mode_requests (conversation_id, request_id, initiator, surface,
                 prior_mode, requested_mode, state, application, requested_at_ms)
             VALUES ('{CONVERSATION}', 'mode-1', 'alice', 'panel', 'ask', 'auto', 'applied',
                 'applied', 1500);"
        ))
        .unwrap();
    if seven {
        connection.execute_batch(SIX_TO_SEVEN).unwrap();
    }
}

fn version(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

/// Every table and index with the statement that made it, which is what
/// "the same shape" means for SQLite.
fn shape(path: &Path) -> Vec<(String, String, Option<String>)> {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT type, name, sql FROM sqlite_schema ORDER BY type, name")
        .unwrap();
    let rows = statement
        .query_map(params![], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

fn current_shape() -> Vec<(String, String, Option<String>)> {
    let (_directory, path) = private_path();
    drop(LocalConversationStore::open(&path).unwrap());
    shape(&path)
}

async fn assert_kept(store: &LocalConversationStore) {
    let id = ConversationId::new(CONVERSATION).unwrap();
    let organization = OrganizationId::new("org").unwrap();
    let alice = PrincipalId::new("alice").unwrap();
    let listed = ConversationListing::list(store, &organization, &alice, false, 10, &Reader::Owner)
        .await
        .unwrap();
    assert_eq!(listed.unreadable, 0);
    let [only] = &listed.conversations[..] else {
        panic!(
            "one conversation listed, not {}",
            listed.conversations.len()
        )
    };
    assert_eq!(only.conversation.id(), &id);
    assert_eq!(
        only.summary.title().map(|title| title.as_str().to_owned()),
        Some("Kept across the upgrade".to_owned())
    );
    let request = ConversationRepository::mode_change(store, &id, "mode-1")
        .await
        .unwrap();
    assert!(request.is_some(), "the settled mode request is kept");
    // The tables the upgrade added answer.
    assert_eq!(
        ConversationRepository::pending_model_change(store, &id)
            .await
            .unwrap(),
        None
    );
    UnfinishedTurns::mark(store, &id).await.unwrap();
    let marked = UnfinishedTurns::marked(store).await.unwrap();
    assert_eq!(marked.len(), 1);
    assert_eq!(marked[0].conversation_id, id);
}

#[tokio::test]
async fn a_version_6_file_is_upgraded_and_keeps_its_conversations() {
    let (_directory, path) = private_path();
    older_file(&path, false);
    assert_eq!(version(&path), 6);

    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(version(&path), 9);
    assert_kept(&store).await;
    drop(store);
    assert_eq!(shape(&path), current_shape());
    // Opened again, it is simply current, and what the upgrade let it write
    // is still there.
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(UnfinishedTurns::marked(&store).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_version_7_file_from_before_model_requests_gains_that_table() {
    let (_directory, path) = private_path();
    older_file(&path, true);
    assert_eq!(version(&path), 7);

    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(version(&path), 9);
    assert_kept(&store).await;
    drop(store);
    assert_eq!(shape(&path), current_shape());
}

#[tokio::test]
async fn a_version_7_file_that_already_has_model_requests_is_raised_as_it_is() {
    let (_directory, path) = private_path();
    {
        // A file the build after #975 wrote: today's tables but
        // `agent_switches`, at version 7.
        drop(LocalConversationStore::open(&path).unwrap());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP TABLE agent_switches")
            .unwrap();
        connection.pragma_update(None, "user_version", 7).unwrap();
    }
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(version(&path), 9);
    drop(store);
    assert_eq!(shape(&path), current_shape());
}

/// A version 8 file, from before switches were journaled, gains
/// `agent_switches` and keeps its conversations.
#[tokio::test]
async fn a_version_8_file_gains_the_agent_switch_journal() {
    let (_directory, path) = private_path();
    older_file(&path, true);
    drop(LocalConversationStore::open(&path).unwrap());
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP INDEX one_pending_agent_switch; DROP TABLE agent_switches")
            .unwrap();
        connection.pragma_update(None, "user_version", 8).unwrap();
    }
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(version(&path), 9);
    assert_kept(&store).await;
    let id = ConversationId::new(CONVERSATION).unwrap();
    assert_eq!(
        ConversationRepository::pending_agent_switch(&store, &id)
            .await
            .unwrap(),
        None
    );
    drop(store);
    assert_eq!(shape(&path), current_shape());
}

/// A file a step cannot apply to is refused and left at its version: here a
/// version 6 file that already has a table version 7 adds.
#[test]
fn a_step_that_fails_leaves_the_file_at_its_version_and_says_which() {
    let (_directory, path) = private_path();
    older_file(&path, false);
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE unfinished_turns (conversation_id TEXT) STRICT;")
        .unwrap();
    assert!(matches!(
        LocalConversationStore::open(&path),
        Err(nessa_local_database::OpenError::Upgrade { from: 6, .. })
    ));
    assert_eq!(version(&path), 6);
    let kept: String = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT title FROM summaries WHERE conversation_id = ?1",
            [CONVERSATION],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, "Kept across the upgrade");
}
