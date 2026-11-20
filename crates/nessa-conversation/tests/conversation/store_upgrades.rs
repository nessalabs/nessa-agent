//! A metadata file an older build wrote is brought forward at open and keeps
//! its rows: version 6 (before `unfinished_turns`), version 7 with or
//! without `model_requests`, which #975 added without raising the version,
//! version 8 (before `agent_switches`), version 9 (before
//! `closed_threads`), version 10 (before `summaries.ask`), and version 11
//! (before `owner_decisions`).
//! `metadata_v6.sql` is version 6's definition exactly as that build shipped
//! it (docs/adr/todo/202-versioned-local-datasets.md, "Migrations").
use super::store::LocalConversationStore;
use crate::application::{ConversationListing, ConversationRepository, Reader, UnfinishedTurns};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{params, Connection};
use nessa_local_database::Schema;
use nessa_protocol::conversation::domain::ConversationId;
use std::path::{Path, PathBuf};

const VERSION_6: &str = include_str!("metadata_v6.sql");

/// The step #974 shipped, which a version 6 file runs to become the version
/// 7 file that build wrote, before #975 added `model_requests`.
const SIX_TO_SEVEN: &str = include_str!("../../src/infrastructure/schema_upgrades/6.sql");

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
    assert_eq!(version(&path), 12);
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
    assert_eq!(version(&path), 12);
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
            .execute_batch(
                "DROP TABLE agent_switches; DROP TABLE closed_threads;
                 DROP TABLE owner_decisions; DROP TABLE decision_settings; ALTER TABLE summaries DROP COLUMN ask",
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 7).unwrap();
    }
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(version(&path), 12);
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
            .execute_batch(
                "DROP INDEX one_pending_agent_switch; DROP TABLE agent_switches;
                 DROP TABLE closed_threads; DROP TABLE owner_decisions; DROP TABLE decision_settings;
                 ALTER TABLE summaries DROP COLUMN ask",
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 8).unwrap();
    }
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(version(&path), 12);
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

/// A version 9 file, from before a thread's close was kept, gains
/// `closed_threads` and keeps its conversations.
#[tokio::test]
async fn a_version_9_file_gains_the_closed_threads() {
    let (_directory, path) = private_path();
    older_file(&path, true);
    drop(LocalConversationStore::open(&path).unwrap());
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE closed_threads; DROP TABLE owner_decisions; DROP TABLE decision_settings;
                 ALTER TABLE summaries DROP COLUMN ask",
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 9).unwrap();
    }
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(version(&path), 12);
    assert_kept(&store).await;
    let id = ConversationId::new(CONVERSATION).unwrap();
    assert!(!ConversationRepository::thread_closed(&store, &id)
        .await
        .unwrap());
    ConversationRepository::close_thread(&store, &id)
        .await
        .unwrap();
    drop(store);
    assert_eq!(shape(&path), current_shape());
    // Kept across a restart.
    let store = LocalConversationStore::open(&path).unwrap();
    assert!(ConversationRepository::thread_closed(&store, &id)
        .await
        .unwrap());
}

/// A version 10 file, from before an agent could ask its owner, gains
/// `summaries.ask` and keeps its conversations, with nothing asked.
#[tokio::test]
async fn a_version_10_file_gains_the_ask_on_its_summaries() {
    use crate::application::ConversationSummaries;
    let (_directory, path) = private_path();
    older_file(&path, true);
    drop(LocalConversationStore::open(&path).unwrap());
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP TABLE owner_decisions; DROP TABLE decision_settings; ALTER TABLE summaries DROP COLUMN ask")
            .unwrap();
        connection.pragma_update(None, "user_version", 10).unwrap();
    }
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(version(&path), 12);
    assert_kept(&store).await;
    let id = ConversationId::new(CONVERSATION).unwrap();
    let kept = ConversationSummaries::load(&store, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.ask(), None);
    let ask = nessa_protocol::conversation::view::OwnerAsk {
        ask_id: "0123456789abcdef0123456789abcdef".into(),
        question: "Ship it?".into(),
        choices: vec![
            nessa_protocol::conversation::view::OwnerAskChoice {
                label: "Yes".into(),
                consequence: "It ships.".into(),
                undo: None,
                effort: None,
                pros: Vec::new(),
                cons: Vec::new(),
            },
            nessa_protocol::conversation::view::OwnerAskChoice {
                label: "No".into(),
                consequence: "It waits.".into(),
                undo: None,
                effort: None,
                pros: Vec::new(),
                cons: Vec::new(),
            },
        ],
        recommended: 0,
        reason: "It is ready.".into(),
        asked_at_ms: 3000,
        access: None,
        summary: None,
        auto_pick: None,
    };
    let asked = nessa_protocol::conversation::domain::ConversationSummary::after_asking(
        Some(&kept),
        ask.clone(),
    );
    ConversationSummaries::record(&store, &id, asked)
        .await
        .unwrap();
    drop(store);
    assert_eq!(shape(&path), current_shape());
    // Kept across a restart, on the summary a list reads.
    let store = LocalConversationStore::open(&path).unwrap();
    let organization = OrganizationId::new("org").unwrap();
    let alice = PrincipalId::new("alice").unwrap();
    let listed =
        ConversationListing::list(&store, &organization, &alice, false, 10, &Reader::Owner)
            .await
            .unwrap();
    assert_eq!(listed.conversations[0].summary.ask(), Some(&ask));
    // A stored ask that no longer reads is left off; the summary still lists.
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE summaries SET ask = '{}' WHERE conversation_id = ?1",
            [CONVERSATION],
        )
        .unwrap();
    let summary = ConversationSummaries::load(&store, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(summary.ask(), None);
    assert_eq!(
        summary.title().map(|title| title.as_str()),
        Some("Kept across the upgrade")
    );
}

fn ask(ask_id: &str, at_ms: u64) -> nessa_protocol::conversation::view::OwnerAsk {
    nessa_protocol::conversation::view::OwnerAsk {
        ask_id: ask_id.into(),
        question: "Ship it?".into(),
        choices: vec![
            nessa_protocol::conversation::view::OwnerAskChoice {
                label: "Yes".into(),
                consequence: "It ships.".into(),
                undo: None,
                effort: None,
                pros: Vec::new(),
                cons: Vec::new(),
            },
            nessa_protocol::conversation::view::OwnerAskChoice {
                label: "No".into(),
                consequence: "It waits.".into(),
                undo: None,
                effort: None,
                pros: Vec::new(),
                cons: Vec::new(),
            },
        ],
        recommended: 0,
        reason: "It is ready.".into(),
        asked_at_ms: at_ms,
        access: None,
        summary: None,
        auto_pick: None,
    }
}

/// A version 11 file, from before decisions were kept, gains
/// `owner_decisions` with the ask its summary waits on as a decision still
/// waiting; the message that answers it then settles it, and the next ask
/// is kept beside it.
#[tokio::test]
async fn a_version_11_file_keeps_the_waiting_ask_as_its_first_decision() {
    use crate::application::ConversationSummaries;
    use nessa_protocol::conversation::domain::ConversationSummary;
    use nessa_protocol::conversation::view::{OwnerDecision, OwnerDecisionOutcome};
    let (_directory, path) = private_path();
    older_file(&path, true);
    let id = ConversationId::new(CONVERSATION).unwrap();
    let first = ask("0123456789abcdef0123456789abcdef", 3000);
    {
        let store = LocalConversationStore::open(&path).unwrap();
        let kept = ConversationSummaries::load(&store, &id)
            .await
            .unwrap()
            .unwrap();
        ConversationSummaries::record(
            &store,
            &id,
            ConversationSummary::after_asking(Some(&kept), first.clone()),
        )
        .await
        .unwrap();
    }
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP TABLE owner_decisions; DROP TABLE decision_settings")
            .unwrap();
        connection.pragma_update(None, "user_version", 11).unwrap();
    }
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(version(&path), 12);
    assert_kept(&store).await;
    assert_eq!(
        ConversationSummaries::decisions(&store, &id).await.unwrap(),
        vec![OwnerDecision {
            ask: first.clone(),
            outcome: None,
            auto_pick_paused: false,
            auto_pick_due_at_ms: None,
        }]
    );
    let waiting = ConversationSummaries::load(&store, &id)
        .await
        .unwrap()
        .unwrap();
    let answered = ConversationSummary::after_message(Some(&waiting), "No. Next week.", None, 4000)
        .answering(Some(&first.ask_id), "No. Next week.", false);
    ConversationSummaries::record(&store, &id, answered.clone())
        .await
        .unwrap();
    let second = ask("fedcba9876543210fedcba9876543210", 5000);
    ConversationSummaries::record(
        &store,
        &id,
        ConversationSummary::after_asking(Some(&answered), second.clone()),
    )
    .await
    .unwrap();
    drop(store);
    assert_eq!(shape(&path), current_shape());
    // Kept across a restart, newest first.
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(
        ConversationSummaries::decisions(&store, &id).await.unwrap(),
        vec![
            OwnerDecision {
                ask: second,
                outcome: None,
                auto_pick_paused: false,
                auto_pick_due_at_ms: None,
            },
            OwnerDecision {
                ask: first,
                outcome: Some(OwnerDecisionOutcome::Decided {
                    text: "No. Next week.".into(),
                    choice: Some(1),
                    at_ms: 4000,
                    auto: false,
                }),
                auto_pick_paused: false,
                auto_pick_due_at_ms: None,
            },
        ]
    );
    // Gone with the summary.
    ConversationSummaries::erase(&store, &id).await.unwrap();
    assert!(ConversationSummaries::decisions(&store, &id)
        .await
        .unwrap()
        .is_empty());
}

/// The worst answer a decision keeps, a message of control characters that
/// JSON writes six bytes each, is still taken by the store with the summary;
/// and a newer ask is listed first even when the clock stepped back, since
/// decisions are listed in the order they were written.
#[tokio::test]
async fn a_control_character_answer_is_kept_and_a_newer_ask_leads_whatever_its_clock() {
    use crate::application::ConversationSummaries;
    use nessa_protocol::conversation::domain::ConversationSummary;
    use nessa_protocol::conversation::view::{OwnerDecisionOutcome, MAX_DECISION_ANSWER_BYTES};
    let (_directory, path) = private_path();
    older_file(&path, true);
    let id = ConversationId::new(CONVERSATION).unwrap();
    let store = LocalConversationStore::open(&path).unwrap();
    let kept = ConversationSummaries::load(&store, &id)
        .await
        .unwrap()
        .unwrap();
    let first = ask("0123456789abcdef0123456789abcdef", 9000);
    let asked = ConversationSummary::after_asking(Some(&kept), first.clone());
    ConversationSummaries::record(&store, &id, asked.clone())
        .await
        .unwrap();
    let worst = "\u{1}".repeat(MAX_DECISION_ANSWER_BYTES * 2);
    let answered = ConversationSummary::after_message(Some(&asked), &worst, None, 9500).answering(
        Some(&first.ask_id),
        &worst,
        false,
    );
    ConversationSummaries::record(&store, &id, answered.clone())
        .await
        .unwrap();
    // Asked by a clock that stepped back.
    let second = ask("fedcba9876543210fedcba9876543210", 1000);
    ConversationSummaries::record(
        &store,
        &id,
        ConversationSummary::after_asking(Some(&answered), second.clone()),
    )
    .await
    .unwrap();
    let decisions = ConversationSummaries::decisions(&store, &id).await.unwrap();
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0].ask, second);
    assert_eq!(decisions[1].ask, first);
    match &decisions[1].outcome {
        Some(OwnerDecisionOutcome::Decided { text, .. }) => {
            assert!(!text.is_empty() && text.len() <= MAX_DECISION_ANSWER_BYTES);
        }
        other => panic!("the answer is kept as decided, not {other:?}"),
    }
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
        Err(crate::infrastructure::ConversationStoreOpenError::Metadata(
            nessa_local_database::OpenError::Upgrade { from: 6, .. }
        ))
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

/// An owner's decision settings are the default until kept, kept per owner,
/// and the file refuses a wait outside the bounds whatever writes it.
#[tokio::test]
async fn decision_settings_are_kept_per_owner_within_their_bounds() {
    use crate::application::ConversationSummaries;
    use nessa_protocol::conversation::view::DecisionSettings;
    let (_directory, path) = private_path();
    let store = LocalConversationStore::open(&path).unwrap();
    let organization = OrganizationId::new("org").unwrap();
    let owner = PrincipalId::new("owner").unwrap();
    let other = PrincipalId::new("other").unwrap();
    assert_eq!(
        store
            .decision_settings(&organization, &owner)
            .await
            .unwrap(),
        DecisionSettings::default()
    );
    let hour = DecisionSettings {
        auto_pick: false,
        auto_pick_after_seconds: 3600,
    };
    store
        .save_decision_settings(&organization, &owner, hour)
        .await
        .unwrap();
    assert_eq!(
        store
            .decision_settings(&organization, &owner)
            .await
            .unwrap(),
        hour
    );
    assert_eq!(
        store
            .decision_settings(&organization, &other)
            .await
            .unwrap(),
        DecisionSettings::default()
    );
    for seconds in [59, 86_401] {
        assert!(store
            .save_decision_settings(
                &organization,
                &owner,
                DecisionSettings {
                    auto_pick: true,
                    auto_pick_after_seconds: seconds,
                },
            )
            .await
            .is_err());
    }
    drop(store);
    let store = LocalConversationStore::open(&path).unwrap();
    assert_eq!(
        store
            .decision_settings(&organization, &owner)
            .await
            .unwrap(),
        hour
    );
}

/// Auto-pick reads each decision still waiting and not paused, with its
/// owner's settings (the default until kept); a pause takes it out until it
/// is lifted, and an answered one is never paused or read again.
#[tokio::test]
async fn auto_pick_reads_the_waiting_decisions_its_owners_did_not_pause() {
    use crate::application::ConversationSummaries;
    use nessa_protocol::conversation::domain::ConversationSummary;
    use nessa_protocol::conversation::view::{AskUndo, AutoPick, DecisionSettings};
    let (_directory, path) = private_path();
    older_file(&path, true);
    let id = ConversationId::new(CONVERSATION).unwrap();
    let organization = OrganizationId::new("org").unwrap();
    let alice = PrincipalId::new("alice").unwrap();
    let store = LocalConversationStore::open(&path).unwrap();
    let kept = ConversationSummaries::load(&store, &id)
        .await
        .unwrap()
        .unwrap();
    let mut first = ask("0123456789abcdef0123456789abcdef", 3000);
    first.auto_pick = Some(AutoPick::Allowed);
    first.choices[0].undo = Some(AskUndo::Easy);
    let asked = ConversationSummary::after_asking(Some(&kept), first.clone());
    ConversationSummaries::record(&store, &id, asked.clone())
        .await
        .unwrap();
    let waiting = store.auto_picks_waiting().await.unwrap();
    let [only] = &waiting[..] else {
        panic!("the one waiting decision is read, not {}", waiting.len());
    };
    assert_eq!(only.conversation, id);
    assert_eq!(only.organization, organization);
    assert_eq!(only.owner, alice);
    assert_eq!(only.ask, first);
    assert_eq!(only.settings, DecisionSettings::default());
    let off = DecisionSettings {
        auto_pick: false,
        auto_pick_after_seconds: 120,
    };
    store
        .save_decision_settings(&organization, &alice, off)
        .await
        .unwrap();
    assert_eq!(store.auto_picks_waiting().await.unwrap()[0].settings, off);
    assert!(store
        .pause_auto_pick(&id, &first.ask_id, Some(3500))
        .await
        .unwrap());
    assert!(store.auto_picks_waiting().await.unwrap().is_empty());
    assert!(ConversationSummaries::decisions(&store, &id).await.unwrap()[0].auto_pick_paused);
    assert!(!store
        .pause_auto_pick(&id, "fedcba9876543210fedcba9876543210", Some(3500))
        .await
        .unwrap());
    assert!(store
        .pause_auto_pick(&id, &first.ask_id, None)
        .await
        .unwrap());
    assert_eq!(store.auto_picks_waiting().await.unwrap().len(), 1);
    let answered = ConversationSummary::after_message(Some(&asked), "Yes.", None, 4000).answering(
        Some(&first.ask_id),
        "Yes.",
        true,
    );
    ConversationSummaries::record(&store, &id, answered)
        .await
        .unwrap();
    assert!(store.auto_picks_waiting().await.unwrap().is_empty());
    assert!(!store
        .pause_auto_pick(&id, &first.ask_id, Some(4500))
        .await
        .unwrap());
}
