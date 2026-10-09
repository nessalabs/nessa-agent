//! Public save publication, exact retry and serialized checkpoint contracts.

use super::opened;
use nessa_sdk::{
    application::agent_execution::sessions::{
        ChangeWatchState, SessionChange, SessionLoadState, SessionSaveUnit, SessionStorage,
        StorageError,
    },
    domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext, SessionId},
    infrastructure::session_storage::{
        RecordStorage, TranscriptCheckpoint, TranscriptError, TranscriptFold,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    },
};
use nessa_sync::replication::{
    application::RecordSource,
    domain::{Id, PageRequest, Record, Scope},
    infrastructure::MAX_PAGE_PAYLOAD,
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    future::Future,
    path::Path,
    task::{Context, Poll, Waker},
};

fn sql(root: &Path, statement: &str) {
    Connection::open(root.join("records.sqlite3"))
        .unwrap()
        .execute_batch(statement)
        .unwrap();
}
fn rows(root: &Path) -> i64 {
    Connection::open(root.join("records.sqlite3"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM event_records", [], |row| row.get(0))
        .unwrap()
}
async fn emitted_records(storage: &RecordStorage, id: &SessionId) -> (Scope, Vec<Record>) {
    let source = storage
        .record_source(id, Id::new("origin").unwrap())
        .await
        .unwrap()
        .unwrap();
    tokio::task::spawn_blocking(move || {
        let mut source = source;
        let scope = source.scope(Id::new("receiver").unwrap(), Id::new("epoch").unwrap());
        let head = source.head(&scope).unwrap();
        let mut records = Vec::new();
        let mut after = 0;
        while after < head {
            let page = source
                .page(&PageRequest {
                    scope: scope.clone(),
                    after,
                    target: head,
                    max_records: 64,
                    max_payload_bytes: MAX_PAGE_PAYLOAD,
                    max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                })
                .unwrap();
            assert!(!page.records.is_empty(), "published suffix must advance");
            after = page.records.last().unwrap().position;
            records.extend(page.records);
        }
        // The final source handle joins its physical worker on this blocking thread.
        drop(source);
        (scope, records)
    })
    .await
    .unwrap()
}
async fn checkpoint_fixture() -> TranscriptFold {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (opening, mut snapshot) = opened(&id);
    let first = lease.load().await.unwrap().binding().clone();
    let receipt = lease
        .save_changes(
            first.clone(),
            snapshot.clone(),
            vec![SessionSaveUnit::new(vec![opening]).unwrap()],
        )
        .await
        .unwrap();
    let next = receipt.next_for(&first, 1).unwrap();
    let context = ProviderContext::Recorded(ExecutionSessionId::new("next-context").unwrap());
    let change = SessionChange::ProviderContext {
        before: ProviderContext::Absent,
        after: context.clone(),
    };
    snapshot.provider_context = context;
    lease
        .save_changes(
            next,
            snapshot.clone(),
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    let (scope, records) = emitted_records(&storage, &id).await;
    let mut fold = TranscriptFold::new(scope).unwrap();
    fold.apply(&records).unwrap();
    assert_eq!(fold.snapshot(), Some(&snapshot));
    drop(lease);
    storage.shutdown().await.unwrap();
    fold
}
fn checkpoint_json(checkpoint: &TranscriptCheckpoint) -> Value {
    let bytes: Vec<u8> = checkpoint.chunks().flatten().copied().collect();
    serde_json::from_slice(&bytes).unwrap()
}
fn changed_checkpoint(value: &Value) -> Result<TranscriptCheckpoint, TranscriptError> {
    TranscriptCheckpoint::from_chunks(vec![serde_json::to_vec(value).unwrap()])
}
fn assert_refused(fold: &TranscriptFold, saved: &Value, path: &str, replacement: Value) {
    let baseline = fold.checkpoint().unwrap();
    assert!(TranscriptFold::restore(fold.scope().clone(), fold.applied(), &baseline).is_ok());
    let mut changed = saved.clone();
    *changed
        .pointer_mut(path)
        .expect("field emitted by the actual checkpoint") = replacement;
    assert!(
        matches!(
            changed_checkpoint(&changed).and_then(|checkpoint| {
                TranscriptFold::restore(fold.scope().clone(), fold.applied(), &checkpoint)
            }),
            Err(TranscriptError::Checkpoint)
        ),
        "accepted serialized contradiction at {path}"
    );
    assert!(TranscriptFold::restore(fold.scope().clone(), fold.applied(), &baseline).is_ok());
}

#[tokio::test]
async fn checkpoint_refuses_lower_prior_base_with_other_completion_fields_unchanged() {
    let fold = checkpoint_fixture().await;
    let saved = checkpoint_json(&fold.checkpoint().unwrap());
    assert_eq!(saved["group"]["identity"]["base"], json!(2));
    assert_refused(&fold, &saved, "/group/identity/base", json!(1));
}
#[tokio::test]
async fn checkpoint_refuses_higher_prior_base_with_other_completion_fields_unchanged() {
    let fold = checkpoint_fixture().await;
    let saved = checkpoint_json(&fold.checkpoint().unwrap());
    assert_eq!(saved["group"]["identity"]["base"], json!(2));
    assert_refused(&fold, &saved, "/group/identity/base", json!(3));
}
#[tokio::test]
async fn checkpoint_refuses_changed_final_unit_preimage_and_completion_chain() {
    let fold = checkpoint_fixture().await;
    let saved = checkpoint_json(&fold.checkpoint().unwrap());
    for path in [
        "/group/unit_previous/0",
        "/group/unit_payload/0",
        "/group/chain/0",
        "/group/unit_length",
        "/group/count",
        "/group/identity/generation",
    ] {
        let value = saved.pointer(path).unwrap().as_u64().unwrap();
        assert_refused(
            &fold,
            &saved,
            path,
            json!(if path.ends_with("/0") {
                (value + 1) % 256
            } else {
                value + 1
            }),
        );
    }
}
#[tokio::test]
async fn checkpoint_rejects_each_independent_completion_binding_contradiction() {
    let fold = checkpoint_fixture().await;
    let saved = checkpoint_json(&fold.checkpoint().unwrap());
    assert_eq!(saved["group"]["count"], json!(1));
    assert_eq!(saved["facts"], json!(2));
    for (path, replacement) in [
        ("/group/count", json!(0)),
        ("/group/count", json!(3)),
        ("/group/identity/base", json!(0)),
        ("/group/identity/base", json!(4)),
        ("/group/identity/stream", json!(vec![2; 32])),
        ("/group/identity/incarnation", json!(vec![2; 16])),
        ("/group", Value::Null),
    ] {
        assert_refused(&fold, &saved, path, replacement);
    }
}
#[tokio::test]
async fn checkpoint_restore_refuses_independent_group_representation_contradictions() {
    let fold = checkpoint_fixture().await;
    let checkpoint = fold.checkpoint().unwrap();
    let valid = checkpoint_json(&checkpoint);
    let mut cases: Vec<(&str, Value)> = Vec::new();
    for path in [
        "/group",
        "/group/identity",
        "/group/chain",
        "/group/unit_previous",
        "/group/unit_payload",
        "/group/identity/stream",
        "/group/identity/incarnation",
        "/group/count",
        "/group/unit_length",
        "/group/identity/base",
        "/group/identity/generation",
    ] {
        cases.push((path, json!("unexpected-string")));
    }
    for path in [
        "/group",
        "/group/identity",
        "/group/count",
        "/group/unit_length",
        "/group/identity/base",
        "/group/identity/generation",
    ] {
        cases.push((path, json!([0])));
    }
    for path in [
        "/group/chain",
        "/group/unit_previous",
        "/group/unit_payload",
        "/group/identity/stream",
    ] {
        cases.push((path, json!(vec![0; 33])));
    }
    cases.push(("/group/identity/incarnation", json!(vec![0; 17])));
    for path in [
        "/group/chain",
        "/group/unit_previous",
        "/group/unit_payload",
        "/group/identity/stream",
        "/group/identity/incarnation",
    ] {
        cases.push((path, json!(["unexpected-string"])));
    }
    for path in [
        "/group/chain",
        "/group/unit_previous",
        "/group/unit_payload",
        "/group/identity/stream",
        "/group/identity/incarnation",
        "/group/count",
        "/group/unit_length",
        "/group/identity/base",
        "/group/identity/generation",
    ] {
        cases.push((path, json!({"unexpected": 0})));
    }
    for (path, replacement) in cases {
        assert_refused(&fold, &valid, path, replacement);
    }
    for path in ["/group", "/group/identity"] {
        let mut malformed = valid.clone();
        malformed
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), json!(0));
        assert!(matches!(
            changed_checkpoint(&malformed).and_then(|checkpoint| {
                TranscriptFold::restore(fold.scope().clone(), fold.applied(), &checkpoint)
            }),
            Err(TranscriptError::Checkpoint)
        ));
    }
    assert!(TranscriptFold::restore(fold.scope().clone(), fold.applied(), &checkpoint).is_ok());
}

#[tokio::test]
async fn completed_prefix_stays_published_during_same_generation_extension_and_checkpoint_restage()
{
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (opening, snapshot) = opened(&id);
    let original = lease.load().await.unwrap().binding().clone();
    let receipt = lease
        .save_changes(
            original.clone(),
            snapshot.clone(),
            vec![SessionSaveUnit::new(vec![opening]).unwrap()],
        )
        .await
        .unwrap();
    let original = receipt.next_for(&original, 1).unwrap();
    let mut watch = storage.watch_committed(&id).unwrap();
    let context = ProviderContext::Recorded(ExecutionSessionId::new("first-context").unwrap());
    let first = SessionChange::ProviderContext {
        before: ProviderContext::Absent,
        after: context.clone(),
    };
    let mut candidate = snapshot.clone();
    candidate.provider_context = context.clone();
    // Refuse the actual next completion insertion after the small Unit is durable.
    sql(&root, "CREATE TRIGGER refuse_completion BEFORE INSERT ON event_records WHEN NEW.offset=X'0000000000000004' BEGIN SELECT RAISE(ABORT, 'fixture completion refusal'); END;");
    assert!(matches!(
        lease
            .save_changes(
                original.clone(),
                candidate,
                vec![SessionSaveUnit::new(vec![first.clone()]).unwrap()]
            )
            .await,
        Err(StorageError::Io(_))
    ));
    assert_eq!(rows(&root), 3);
    assert_eq!(
        lease.load().await.unwrap().state(),
        SessionLoadState::Unfinished
    );
    let prior = storage.read_committed(id.clone()).await.unwrap().unwrap();
    assert_eq!(prior.position(), 2);
    assert_eq!(prior.snapshot(), Some(&snapshot));
    let mut wait = Box::pin(watch.changed());
    assert!(matches!(
        wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    drop(wait);
    sql(&root, "DROP TRIGGER refuse_completion;");
    drop(lease);
    let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
    let loaded = lease.load().await.unwrap();
    assert_eq!(loaded.binding(), &original);
    assert_eq!(loaded.snapshot(), Some(&snapshot));
    let final_context =
        ProviderContext::Recorded(ExecutionSessionId::new("extended-context").unwrap());
    let second = SessionChange::ProviderContext {
        before: context,
        after: final_context.clone(),
    };
    let mut extended = snapshot;
    extended.provider_context = final_context;
    let units = vec![
        SessionSaveUnit::new(vec![first]).unwrap(),
        SessionSaveUnit::new(vec![second]).unwrap(),
    ];
    let receipt = lease
        .save_changes(original.clone(), extended.clone(), units.clone())
        .await
        .unwrap();
    assert_eq!(rows(&root), 5);
    let mut wait = Box::pin(watch.changed());
    assert_eq!(
        wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(ChangeWatchState::Dirty)
    );
    drop(wait);
    assert_eq!(
        lease
            .save_changes(original, extended.clone(), units)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(rows(&root), 5);
    let (scope, records) = emitted_records(&storage, &id).await;
    let mut fold = TranscriptFold::new(scope.clone()).unwrap();
    fold.apply(&records).unwrap();
    assert_eq!(fold.snapshot(), Some(&extended));
    let restored =
        TranscriptFold::restore(scope, fold.applied(), &fold.checkpoint().unwrap()).unwrap();
    assert_eq!(restored.snapshot(), Some(&extended));
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn physical_conflict_fences_live_load_and_later_saves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (change, candidate) = opened(&id);
    let binding = lease.load().await.unwrap().binding().clone();
    let units = vec![SessionSaveUnit::new(vec![change]).unwrap()];
    let receipt = lease
        .save_changes(binding.clone(), candidate.clone(), units.clone())
        .await
        .unwrap();
    let database = Connection::open(root.join("records.sqlite3")).unwrap();
    let actual: Vec<u8> = database
        .query_row(
            "SELECT payload FROM event_records WHERE offset=?1",
            [1u64.to_be_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        database
            .execute(
                "UPDATE event_records SET payload=?1 WHERE offset=?2",
                rusqlite::params![&[99u8][..], 1u64.to_be_bytes().as_slice()]
            )
            .unwrap(),
        1
    );
    assert!(matches!(
        lease
            .save_changes(binding.clone(), candidate.clone(), units.clone())
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(rows(&root), 2);
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(
        database
            .execute(
                "UPDATE event_records SET payload=?1 WHERE offset=?2",
                rusqlite::params![actual, 1u64.to_be_bytes().as_slice()]
            )
            .unwrap(),
        1
    );
    assert!(matches!(
        lease
            .save_changes(binding.clone(), candidate.clone(), units.clone())
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    drop(database);
    drop(lease);
    let lease = storage.open_existing(id).await.unwrap().unwrap();
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&candidate));
    assert_eq!(
        lease.save_changes(binding, candidate, units).await.unwrap(),
        receipt
    );
    assert_eq!(rows(&root), 2);
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn emitted_record_rejection_preserves_public_projection_and_valid_completion() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (opening, snapshot) = opened(&id);
    let binding = lease.load().await.unwrap().binding().clone();
    lease
        .save_changes(
            binding,
            snapshot.clone(),
            vec![SessionSaveUnit::new(vec![opening]).unwrap()],
        )
        .await
        .unwrap();
    let (scope, records) = emitted_records(&storage, &id).await;
    assert_eq!(records.len(), 2);
    let mut fold = TranscriptFold::new(scope.clone()).unwrap();
    fold.apply(&records[..1]).unwrap();
    assert_eq!(fold.applied(), 0);
    assert!(fold.snapshot().is_none());
    let before = fold.checkpoint().unwrap();
    let downloaded = fold.downloaded();
    let mut malformed = records[1].clone();
    malformed.payload.truncate(1);
    fold.apply(&[malformed]).unwrap();
    assert!(fold.snapshot().is_none());
    assert!(fold.downloaded() > downloaded);
    assert_eq!(fold.unreadable().len(), 1);
    assert_eq!(fold.apply(&records[1..]), Err(TranscriptError::Position));
    drop(before);
    drop(lease);
    storage.shutdown().await.unwrap();
}
