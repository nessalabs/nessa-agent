use super::*;
use crate::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{records, SessionChange},
    },
    domain::agent_execution::sessions::ExecutionSessionId,
};
use event_stream::{
    infrastructure::{SqliteOptions, SqliteStore},
    EventConfig, EventReader, PersistenceProfile, Runtime, RuntimeConfig, StreamId,
};
use std::time::Duration;

#[test]
fn retain_next_fact_holds_one_oversized_fact_and_splits_before_a_second() {
    let ceiling = super::super::MAX_STORED_RECORD_BYTES;
    assert!(retain_next_fact(0, ceiling + 1));
    assert!(retain_next_fact(32, 32));
    assert!(!retain_next_fact(ceiling, 1));
    assert!(!retain_next_fact(ceiling / 2 + 1, ceiling / 2 + 1));
}

#[tokio::test]
async fn sqlite_restart_replays_committed_decisions_without_provider_effects() {
    let directory = tempfile::tempdir().unwrap();
    let options = SqliteOptions::new(directory.path().join("records.sqlite3"));
    let config = RuntimeConfig {
        events: EventConfig {
            max_bytes: 1024 * 1024,
            minimum_persistence: PersistenceProfile::ProcessRestart,
        },
        ..RuntimeConfig::default()
    };
    let id = SessionId::new("conversation").unwrap();
    let stream_id = StreamId::new("conversation").unwrap();
    let runtime = Runtime::<SqliteStore>::open(options.clone(), config.clone())
        .await
        .unwrap();
    let stream = runtime.create_stream(&stream_id).await.unwrap();
    let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
        .await
        .unwrap();
    assert!(writer.snapshot().is_none());
    let opened = SessionChange::Opened {
        id: id.clone(),
        provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
        context: ProviderContext::Absent,
    };
    let first = records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
    writer
        .save(
            &runtime,
            None,
            writer.readable_snapshot().unwrap().binding().clone(),
            &first,
            &[SessionSaveUnit::new(vec![opened]).unwrap()],
        )
        .await
        .unwrap();
    let next = writer.readable_snapshot().unwrap().binding().clone();
    let context = SessionChange::ProviderContext {
        before: ProviderContext::Absent,
        after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
    };
    let second = records::fold_changes(Some(&first), std::slice::from_ref(&context)).unwrap();
    writer
        .save(
            &runtime,
            None,
            next.clone(),
            &second,
            &[SessionSaveUnit::new(vec![context]).unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(writer.snapshot(), Some(&second));
    drop(writer);
    assert!(
        runtime
            .shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .closed
    );

    let reopened = Runtime::<SqliteStore>::open(options, config).await.unwrap();
    let same_stream = reopened.create_stream(&stream_id).await.unwrap();
    assert_eq!(same_stream, stream);
    let restored = RecordWriter::replay(&reopened, id, same_stream)
        .await
        .unwrap();
    assert_eq!(restored.snapshot(), Some(&second));
    assert!(
        reopened
            .shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .closed
    );
}

// Ordering cases for immutable save ownership (all use actual grouped SQLite facts):
// | Order | Required observation | Test |
// | New binding -> unit -> completion | Baseline shares old publication; staging/load remain owned | live_save_shares_baseline_and_keeps_owned_loads |
// | Unit -> rejected completion -> exact retry | Old publication survives; retry retains original baseline | failed_completion_keeps_old_publication_and_original_retry_baseline |
// | Completed prefix -> same-binding extension -> replay | Original baseline survives later publication; replay is equivalent | completed_prefix_extension_replays_with_the_original_baseline |

use crate::{
    application::agent_execution::{
        executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode},
        permissions::ActionContext,
        sessions::{InvocationRecord, SubmissionAcknowledgement},
    },
    domain::agent_execution::{
        executions::{ExecutionId, Initiation, MessageChunk, MESSAGE_CLONES},
        prompts::{PromptText, UserMessage},
    },
};

const HISTORY_TEXT: &str = "retained provider output";

async fn populated_writer(path: &std::path::Path) -> (Runtime<SqliteStore>, RecordWriter) {
    let runtime = Runtime::<SqliteStore>::open(
        SqliteOptions::new(path),
        RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        },
    )
    .await
    .unwrap();
    let stream = runtime
        .create_stream(&StreamId::new("conversation").unwrap())
        .await
        .unwrap();
    let id = SessionId::new("conversation").unwrap();
    let mut writer = RecordWriter::replay(&runtime, id.clone(), stream)
        .await
        .unwrap();
    let execution = ExecutionId::new("execution").unwrap();
    let changes = vec![
        SessionChange::Opened {
            id,
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        },
        SessionChange::InputAccepted(Box::new(InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: execution.clone(),
                user_message: UserMessage::text_only(PromptText::new("question").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            actor: ActionContext::new("user", "surface", "request").unwrap(),
            initiation: Initiation::owner(),
            notes: Vec::new(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        })),
        SessionChange::ProviderObservation(ExecutionEvent::new(
            execution,
            ExecutionUpdate::Message(MessageChunk::text(HISTORY_TEXT)),
        )),
    ];
    let observed = records::fold_changes(None, &changes).unwrap();
    writer
        .save(
            &runtime,
            None,
            writer.next.clone(),
            &observed,
            &[SessionSaveUnit::new(changes).unwrap()],
        )
        .await
        .unwrap();
    (runtime, writer)
}

fn context_change(before: &str, after: &str) -> SessionChange {
    SessionChange::ProviderContext {
        before: ProviderContext::Recorded(ExecutionSessionId::new(before).unwrap()),
        after: ProviderContext::Recorded(ExecutionSessionId::new(after).unwrap()),
    }
}

fn assert_history_copies(count: usize) {
    assert_eq!(
        MESSAGE_CLONES.with(|counter| counter.get()),
        (count, count * HISTORY_TEXT.len())
    );
}

#[tokio::test]
async fn live_save_shares_baseline_and_keeps_owned_loads() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, mut writer) = populated_writer(&directory.path().join("records.sqlite3")).await;
    let old = writer.committed.as_ref().unwrap().clone();
    let change = context_change("remote", "second");
    let observed = records::fold_changes(Some(&old), std::slice::from_ref(&change)).unwrap();
    let original = writer.next.clone();
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let receipt = writer
        .save(
            &runtime,
            None,
            original.clone(),
            &observed,
            &[SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    // Three owned continuations remain: preflight, staging, acknowledged publication.
    // Retaining the baseline must not add a fourth copy of the entire history.
    assert_history_copies(3);
    assert!(Arc::ptr_eq(writer.baseline.as_ref().unwrap(), &old));
    assert!(!Arc::ptr_eq(writer.committed.as_ref().unwrap(), &old));
    assert_eq!(writer.snapshot(), Some(&observed));
    assert_eq!(
        old.provider_context,
        ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap())
    );
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let load = writer.readable_snapshot().unwrap();
    assert_history_copies(1);
    assert_eq!(load.snapshot(), Some(&observed));
    assert_eq!(load.binding(), receipt.next());
    assert_eq!(load.state(), SessionLoadState::Published);
    let (owned, binding) = load.into_published(&writer.id).unwrap();
    let mut owned = owned.unwrap();
    owned.invocations.clear();
    owned.provider_context = ProviderContext::Absent;
    assert_eq!(binding, receipt.next().clone());
    assert_eq!(writer.snapshot(), Some(&observed));
    assert_eq!(writer.staged.snapshot.as_ref(), Some(&observed));
    assert_eq!(writer.baseline.as_deref(), Some(old.as_ref()));
    assert_eq!(old.invocations[0].events.len(), 1);
    assert!(
        runtime
            .shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .closed
    );
}

#[tokio::test]
async fn failed_completion_keeps_old_publication_and_original_retry_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("records.sqlite3");
    let (runtime, mut writer) = populated_writer(&path).await;
    let old = writer.committed.as_ref().unwrap().clone();
    let original = writer.next.clone();
    let change = context_change("remote", "second");
    let observed = records::fold_changes(Some(&old), std::slice::from_ref(&change)).unwrap();
    let units = vec![SessionSaveUnit::new(vec![change]).unwrap()];
    // This small context unit and its completion each fit one actual inline fact.
    let terminal = writer.cursor.offset + 2;
    rusqlite::Connection::open(&path).unwrap().execute_batch(&format!(
        "CREATE TRIGGER reject_completion BEFORE INSERT ON event_records WHEN NEW.offset=X'{terminal:016X}' BEGIN SELECT RAISE(ABORT, 'completion rejected'); END;"
    )).unwrap();
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    assert!(matches!(
        writer
            .save(&runtime, None, original.clone(), &observed, &units)
            .await,
        Err(StorageError::Io(_))
    ));
    assert_history_copies(2); // Preflight + private staging; no premature publication copy.
    assert!(Arc::ptr_eq(writer.committed.as_ref().unwrap(), &old));
    assert!(Arc::ptr_eq(writer.baseline.as_ref().unwrap(), &old));
    let load = writer.readable_snapshot().unwrap();
    assert_eq!(load.snapshot(), Some(old.as_ref()));
    assert_eq!(load.binding(), &original);
    assert_eq!(load.state(), SessionLoadState::Unfinished);
    assert_eq!(writer.staged.snapshot.as_ref(), Some(&observed));
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TRIGGER reject_completion")
        .unwrap();
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let receipt = writer
        .save(&runtime, None, original.clone(), &observed, &units)
        .await
        .unwrap();
    assert_history_copies(2); // Retry preflight + acknowledged publication; baseline stays original.
    assert!(Arc::ptr_eq(writer.baseline.as_ref().unwrap(), &old));
    assert_eq!(writer.snapshot(), Some(&observed));
    let published = writer.committed.as_ref().unwrap().clone();
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    assert_eq!(
        writer
            .save(&runtime, None, original, &observed, &units)
            .await
            .unwrap(),
        receipt
    );
    assert_history_copies(1); // Completed exact retry validates; it does not publish again.
    assert!(Arc::ptr_eq(writer.committed.as_ref().unwrap(), &published));
    let replayed = RecordWriter::replay(&runtime, writer.id.clone(), writer.stream.clone())
        .await
        .unwrap();
    assert_eq!(replayed.snapshot(), Some(&observed));
    assert_eq!(replayed.next, receipt.next().clone());
    assert!(
        runtime
            .shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .closed
    );
}

#[tokio::test]
async fn completed_prefix_extension_replays_with_the_original_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, mut writer) = populated_writer(&directory.path().join("records.sqlite3")).await;
    let old = writer.committed.as_ref().unwrap().clone();
    let original = writer.next.clone();
    let first = context_change("remote", "second");
    let second = context_change("second", "third");
    let prefix = records::fold_changes(Some(&old), std::slice::from_ref(&first)).unwrap();
    let final_state = records::fold_changes(Some(&prefix), std::slice::from_ref(&second)).unwrap();
    writer
        .save(
            &runtime,
            None,
            original.clone(),
            &prefix,
            &[SessionSaveUnit::new(vec![first.clone()]).unwrap()],
        )
        .await
        .unwrap();
    let published_prefix = writer.committed.as_ref().unwrap().clone();
    let units = vec![
        SessionSaveUnit::new(vec![first]).unwrap(),
        SessionSaveUnit::new(vec![second]).unwrap(),
    ];
    MESSAGE_CLONES.with(|counter| counter.set((0, 0)));
    let receipt = writer
        .save(&runtime, None, original, &final_state, &units)
        .await
        .unwrap();
    assert_history_copies(2); // Original-baseline preflight + new completion, no restaging/reset.
    assert!(Arc::ptr_eq(writer.baseline.as_ref().unwrap(), &old));
    assert_eq!(*published_prefix, prefix);
    assert_eq!(writer.snapshot(), Some(&final_state));
    let replayed = RecordWriter::replay(&runtime, writer.id.clone(), writer.stream.clone())
        .await
        .unwrap();
    assert_eq!(replayed.snapshot(), Some(&final_state));
    assert_eq!(replayed.baseline.as_deref(), Some(old.as_ref()));
    assert_eq!(replayed.next, receipt.next().clone());
    assert!(
        runtime
            .shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .closed
    );
}
