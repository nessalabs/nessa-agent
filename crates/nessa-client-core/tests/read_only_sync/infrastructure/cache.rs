//! Private SQLite evidence exercised with SDK-produced physical records.
use super::{fixtures::*, rows, ReadOnlyCache};
use crate::read_only_sync::{
    application::{CacheError, CachePolicy},
    domain::CacheReset,
};
use nessa_local_database::rusqlite::params;
use nessa_sdk::{
    application::agent_execution::sessions::{CommittedViewState, StorageError},
    infrastructure::session_storage::{
        physical_record_schema, TranscriptError, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    },
};
use nessa_sync::replication::{
    application::{ReplicaStore, StoreError},
    catalogue::CatalogueStore,
    domain::{Checkpoint, Limits, Record, Scope},
    infrastructure::{MAX_PAGE_PAYLOAD, MAX_PAGE_RECORDS},
};
use std::sync::Arc;

struct ResetEvidence {
    cause: String,
    initiator: String,
    caller: String,
    downloaded: Vec<u8>,
    applied: Vec<u8>,
    facts: Vec<u8>,
    generation: Vec<u8>,
    observed_at_ms: Vec<u8>,
}

#[test]
fn sdk_decision_refusal_retains_application_cause() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cause.sqlite3"));
    let cause = StorageError::Corrupt("checkpoint too large; diagnostic only".to_owned());
    let refused = super::records::transcript_error(TranscriptError::Decision(cause.clone()));
    assert_eq!(cache.remember_refusal(refused), StoreError::Failed);
    assert_eq!(cache.take_refusal(), Some(CacheError::Transcript(cause)));
    assert_eq!(cache.cached_progress(&scope()).unwrap(), None);
}

#[test]
fn physical_cache_policy_refuses_before_database_open() {
    let root = tempfile::tempdir().unwrap();
    let base = policy();
    for (index, limits) in [
        Limits::new(
            MAX_PAGE_RECORDS + 1,
            MAX_PAGE_PAYLOAD,
            MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        ),
        Limits::new(1, MAX_PAGE_PAYLOAD + 1, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES),
        Limits::new(1, MAX_PAGE_PAYLOAD, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES + 1),
    ]
    .into_iter()
    .enumerate()
    {
        let policy = CachePolicy::new(
            base.database_bytes(),
            base.checkpoint_bytes(),
            limits.unwrap(),
        )
        .unwrap();
        let path = cache_path(root.path(), &format!("invalid-{index}.sqlite3"));
        assert!(matches!(
            ReadOnlyCache::open(&path, policy, Arc::new(FixedClock)),
            Err(CacheError::InvalidPolicy)
        ));
        assert!(!path.exists());
    }
    let path = cache_path(root.path(), "valid.sqlite3");
    assert!(ReadOnlyCache::open(&path, base, Arc::new(FixedClock)).is_ok());
}

#[test]
fn fresh_offline_cache_is_not_empty() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    assert_eq!(cache.load(&scope()).unwrap(), None);
    assert_eq!(
        cache.transcript(&scope()).unwrap().view_state(),
        CommittedViewState::NotLoaded
    );
}

#[test]
fn head_observation_uses_shared_freshness_owner() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut first = cache(&path);
    first.observe_head(&scope(), 0).unwrap();
    assert_eq!(
        first.transcript(&scope()).unwrap().view_state(),
        CommittedViewState::CompleteEmpty
    );
    let saved = first.cached_progress(&scope()).unwrap();
    drop(first);
    let mut reopened = cache(&path);
    assert_eq!(
        reopened.transcript(&scope()).unwrap().view_state(),
        CommittedViewState::Stale
    );
    reopened.observe_head(&scope(), 0).unwrap();
    assert_eq!(reopened.cached_progress(&scope()).unwrap(), saved);
    assert_eq!(
        reopened.transcript(&scope()).unwrap().view_state(),
        CommittedViewState::CompleteEmpty
    );
    reopened.mark_unknown(&scope()).unwrap();
    assert_eq!(
        reopened.transcript(&scope()).unwrap().view_state(),
        CommittedViewState::Unknown
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_restores_pending_suffix_once() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    assert!(records.len() > 3);
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut first = cache(&path);
    first.apply(plan(&scope, 0, records[..2].to_vec())).unwrap();
    let pending = first.cached_progress(&scope).unwrap().unwrap();
    assert_eq!(pending.downloaded, 2);
    assert_eq!(pending.applied, 0);
    assert_eq!(pending.facts, 0);
    drop(first);
    let mut reopened = cache(&path);
    assert_eq!(reopened.cached_progress(&scope).unwrap(), Some(pending));
    assert_eq!(reopened.transcript(&scope).unwrap().downloaded(), 2);
    assert!(reopened.transcript(&scope).unwrap().snapshot().is_none());
    let mut after = 2;
    for page in records[2..].chunks(16) {
        reopened.apply(plan(&scope, after, page.to_vec())).unwrap();
        after = page.last().unwrap().position;
    }
    let progress = reopened.cached_progress(&scope).unwrap().unwrap();
    assert_eq!(progress.downloaded, records.last().unwrap().position);
    assert_eq!(progress.applied, progress.downloaded);
    assert_eq!(progress.facts, 1);
    assert!(reopened.transcript(&scope).unwrap().snapshot().is_some());
    drop(reopened);
    let mut final_cache = cache(&path);
    assert_eq!(final_cache.cached_progress(&scope).unwrap(), Some(progress));
    assert!(final_cache.transcript(&scope).unwrap().snapshot().is_some());
}

#[test]
fn invalid_semantic_page_has_no_effects() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    cache.observe_head(&scope(), 0).unwrap();
    let before = cache.cached_progress(&scope()).unwrap();
    let invalid = Record {
        position: 1,
        id: id("invalid"),
        scope: scope(),
        payload: vec![255],
    };
    assert_eq!(
        cache.apply(plan(&scope(), 0, vec![invalid])),
        Err(StoreError::Failed)
    );
    assert_eq!(cache.take_refusal(), Some(CacheError::Corrupt));
    assert_eq!(cache.cached_progress(&scope()).unwrap(), before);
    let count: i64 = cache
        .connection
        .query_row("SELECT count(*) FROM transcript_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_commit_reply_reconciles_exact_slice() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let path = cache_path(root.path(), "cache.sqlite3");
    let commit = plan(&scope, 0, records[..2].to_vec());
    let mut first = cache(&path);
    first.apply(commit.clone()).unwrap();
    let saved = first.cached_progress(&scope).unwrap();
    drop(first);
    let mut reopened = cache(&path);
    reopened.apply(commit.clone()).unwrap();
    assert_eq!(reopened.cached_progress(&scope).unwrap(), saved);
    let mut conflicting = commit.records().to_vec();
    conflicting[0].payload.push(0);
    assert_eq!(
        reopened.apply(plan(&scope, 0, conflicting)),
        Err(StoreError::ConflictingRecord)
    );
    assert_eq!(reopened.cached_progress(&scope).unwrap(), saved);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_commit_and_checkpoint_are_atomic() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut first = cache(&path);
    first.apply(plan(&scope, 0, records[..2].to_vec())).unwrap();
    let before = first.cached_progress(&scope).unwrap();
    first
        .connection
        .execute_batch(
            "CREATE TRIGGER refuse_next_record BEFORE INSERT ON transcript_records
         BEGIN SELECT RAISE(ABORT, 'injected transaction failure'); END;",
        )
        .unwrap();
    assert_eq!(
        first.apply(plan(&scope, 2, records[2..].to_vec())),
        Err(StoreError::Failed)
    );
    assert_eq!(first.take_refusal(), Some(CacheError::Unavailable));
    assert_eq!(first.cached_progress(&scope).unwrap(), before);
    drop(first);
    let mut reopened = cache(&path);
    assert_eq!(reopened.cached_progress(&scope).unwrap(), before);
    assert!(reopened.transcript(&scope).unwrap().snapshot().is_none());
    reopened
        .connection
        .execute_batch("DROP TRIGGER refuse_next_record")
        .unwrap();
    reopened
        .apply(plan(&scope, 2, records[2..].to_vec()))
        .unwrap();
    assert!(reopened.transcript(&scope).unwrap().snapshot().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_writer_requires_reload() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut delayed = cache(&path);
    let mut winner = cache(&path);
    assert_eq!(delayed.load(&scope).unwrap(), None);
    let old_plan = plan(&scope, 0, records.clone());
    winner
        .apply(plan(&scope, 0, records[..2].to_vec()))
        .unwrap();
    assert_eq!(delayed.apply(old_plan), Err(StoreError::Stale));
    assert_eq!(delayed.take_refusal(), Some(CacheError::Stale));
    assert_eq!(delayed.load(&scope).unwrap().unwrap().position(), 2);
    delayed
        .apply(plan(&scope, 2, records[2..].to_vec()))
        .unwrap();
    assert_eq!(
        winner.cached_progress(&scope).unwrap(),
        delayed.cached_progress(&scope).unwrap()
    );
    assert!(winner.transcript(&scope).unwrap().snapshot().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_position_generation_change_refuses_delayed_plan() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let path = cache_path(root.path(), "generation.sqlite3");
    let mut delayed = cache(&path);
    delayed
        .apply(plan(&scope, 0, records[..2].to_vec()))
        .unwrap();
    let admitted = delayed.cached_progress(&scope).unwrap().unwrap();
    let winner = cache(&path);
    winner
        .connection
        .execute(
            "UPDATE transcript_progress SET generation = ?1",
            params![(admitted.generation + 1).to_be_bytes().as_slice()],
        )
        .unwrap();
    assert_eq!(
        delayed.apply(plan(&scope, 2, records[2..].to_vec())),
        Err(StoreError::Stale)
    );
    assert_eq!(delayed.take_refusal(), Some(CacheError::Stale));
    let actual = delayed.cached_progress(&scope).unwrap().unwrap();
    assert_eq!(actual.downloaded, admitted.downloaded);
    assert_eq!(actual.applied, admitted.applied);
    assert_eq!(actual.generation, admitted.generation + 1);
    assert!(delayed.transcript(&scope).unwrap().snapshot().is_none());
    delayed
        .apply(plan(&scope, 2, records[2..].to_vec()))
        .unwrap();
    assert!(delayed.transcript(&scope).unwrap().snapshot().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_identity_cannot_change_meaning() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    cache.apply(plan(&scope, 0, records[..2].to_vec())).unwrap();
    let before = cache.cached_progress(&scope).unwrap();
    let mut reused = records[2].clone();
    reused.id = records[0].id.clone();
    assert_eq!(
        cache.apply(plan(&scope, 2, vec![reused])),
        Err(StoreError::ConflictingRecord)
    );
    assert_eq!(cache.cached_progress(&scope).unwrap(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_quota_does_not_acknowledge_work() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let path = cache_path(root.path(), "checkpoint-quota.sqlite3");
    let tiny = CachePolicy::new(policy().database_bytes(), 1, policy().suffix_page()).unwrap();
    let mut cache = ReadOnlyCache::open(&path, tiny, Arc::new(FixedClock)).unwrap();
    assert_eq!(
        cache.apply(plan(&scope, 0, records[..2].to_vec())),
        Err(StoreError::Failed)
    );
    assert_eq!(cache.take_refusal(), Some(CacheError::Quota));
    assert_eq!(cache.load(&scope).unwrap(), None);
    assert_eq!(cache.transcript(&scope).unwrap().downloaded(), 0);

    let disk_path = cache_path(root.path(), "disk-quota.sqlite3");
    let empty = ReadOnlyCache::open(&disk_path, policy(), Arc::new(FixedClock)).unwrap();
    let page_count: i64 = empty
        .connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .unwrap();
    let page_size: i64 = empty
        .connection
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .unwrap();
    drop(empty);
    let disk = CachePolicy::new(
        u64::try_from(page_count * page_size).unwrap(),
        policy().checkpoint_bytes(),
        policy().suffix_page(),
    )
    .unwrap();
    let mut cache = ReadOnlyCache::open(&disk_path, disk, Arc::new(FixedClock)).unwrap();
    assert_eq!(
        cache.apply(plan(&scope, 0, records[..2].to_vec())),
        Err(StoreError::Failed)
    );
    assert_eq!(cache.take_refusal(), Some(CacheError::Quota));
    assert_eq!(cache.load(&scope).unwrap(), None);
    drop(cache);
    let mut reopened = ReadOnlyCache::open(&disk_path, disk, Arc::new(FixedClock)).unwrap();
    assert_eq!(reopened.load(&scope).unwrap(), None);
}

/// An ordinal-tampered checkpoint is disposable cache. The rows are dropped
/// and a later head observation rebuilds an empty fold.
#[test]
fn a_tampered_checkpoint_is_dropped_and_rebuilt() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "cache.sqlite3");
    let mut first = cache(&path);
    first.observe_head(&scope(), 0).unwrap();
    first
        .connection
        .execute("UPDATE transcript_checkpoints SET ordinal = 1", [])
        .unwrap();
    drop(first);
    let mut reopened = cache(&path);
    assert_eq!(reopened.load(&scope()).unwrap(), None);
    assert_eq!(reopened.take_refusal(), None);
    assert_eq!(table_rows(&reopened, "transcript_checkpoints"), 0);
    assert_eq!(table_rows(&reopened, "transcript_progress"), 0);
    reopened.observe_head(&scope(), 0).unwrap();
    assert_eq!(
        reopened.load(&scope()).unwrap(),
        Some(Checkpoint::new(scope(), 0))
    );
}

fn table_rows(cache: &ReadOnlyCache, table: &str) -> i64 {
    cache
        .connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn unreadable_checkpoint_rebuilds(name: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), &format!("{name}.sqlite3"));
    let mut first = cache(&path);
    first.observe_head(&scope(), 0).unwrap();
    let original: Vec<u8> = first
        .connection
        .query_row("SELECT payload FROM transcript_checkpoints", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    edit(&mut value);
    let payload = serde_json::to_vec(&value).unwrap();
    first
        .connection
        .execute(
            "UPDATE transcript_checkpoints SET payload = ?1",
            params![&payload],
        )
        .unwrap();
    drop(first);
    let mut reopened = cache(&path);
    assert_eq!(reopened.load(&scope()).unwrap(), None, "{name}");
    assert_eq!(reopened.take_refusal(), None, "{name}");
    assert_eq!(table_rows(&reopened, "transcript_checkpoints"), 0, "{name}");
    assert_eq!(table_rows(&reopened, "transcript_progress"), 0, "{name}");
    assert_eq!(table_rows(&reopened, "transcript_records"), 0, "{name}");
    reopened.observe_head(&scope(), 0).unwrap();
    assert_eq!(
        reopened.load(&scope()).unwrap(),
        Some(Checkpoint::new(scope(), 0)),
        "{name}"
    );
    let rebuilt: Vec<u8> = reopened
        .connection
        .query_row("SELECT payload FROM transcript_checkpoints", [], |row| {
            row.get(0)
        })
        .unwrap();
    let rebuilt: serde_json::Value = serde_json::from_slice(&rebuilt).unwrap();
    assert_eq!(
        rebuilt["schemaVersion"],
        StorageError::SCHEMA_VERSION,
        "{name}"
    );
}

/// A cached checkpoint this build cannot read is dropped. The load succeeds
/// with no progress, and a later head observation rebuilds the checkpoint.
/// An ordinal-tampered checkpoint is dropped the same way.
#[test]
fn an_unreadable_checkpoint_is_dropped_and_rebuilt() {
    unreadable_checkpoint_rebuilds("future", |value| {
        value["schemaVersion"] = serde_json::json!(StorageError::SCHEMA_VERSION + 1);
    });
    unreadable_checkpoint_rebuilds("unmarked-today", |value| {
        assert_eq!(value["schemaVersion"], StorageError::SCHEMA_VERSION);
        value.as_object_mut().unwrap().remove("schemaVersion");
    });
    unreadable_checkpoint_rebuilds("earlier-shape", |value| {
        value.as_object_mut().unwrap().remove("schemaVersion");
        value["snapshot"] = serde_json::json!("not a snapshot");
    });
    unreadable_checkpoint_rebuilds("broken-body", |value| {
        value["snapshot"] = serde_json::json!("not a snapshot");
    });

    let root = tempfile::tempdir().unwrap();
    let shared = cache_path(root.path(), "shared.sqlite3");
    let mut first = cache(&shared);
    let other = Scope::new(
        id("receiver"),
        id("origin"),
        id("00000000-0000-0000-0000-000000000002"),
        id("incarnation"),
        physical_record_schema(),
        id("epoch"),
    );
    first.observe_head(&scope(), 0).unwrap();
    first.observe_head(&other, 0).unwrap();
    let original: Vec<u8> = first
        .connection
        .query_row(
            "SELECT payload FROM transcript_checkpoints WHERE stream = ?1",
            params![scope().stream().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    value["schemaVersion"] = serde_json::json!(StorageError::SCHEMA_VERSION + 1);
    first
        .connection
        .execute(
            "UPDATE transcript_checkpoints SET payload = ?1 WHERE stream = ?2",
            params![
                serde_json::to_vec(&value).unwrap(),
                scope().stream().as_str()
            ],
        )
        .unwrap();
    drop(first);
    let mut reopened = cache(&shared);
    assert_eq!(reopened.load(&scope()).unwrap(), None);
    assert_eq!(
        reopened.load(&other).unwrap(),
        Some(Checkpoint::new(other.clone(), 0))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_saved_identity_is_refused_before_text_acquisition() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "progress-identity.sqlite3");
    let mut first = cache(&path);
    first.observe_head(&scope(), 0).unwrap();
    let oversized = "x".repeat(rows::MAX_STORED_ID_BYTES + 1);
    first
        .connection
        .execute(
            "UPDATE transcript_progress SET incarnation = ?1",
            params![&oversized],
        )
        .unwrap();
    drop(first);
    let mut reopened = cache(&path);
    rows::IDENTIFIER_TEXT_READS.with(|count| count.set(0));
    assert_eq!(reopened.load(&scope()), Err(StoreError::Failed));
    assert_eq!(reopened.take_refusal(), Some(CacheError::Corrupt));
    rows::IDENTIFIER_TEXT_READS.with(|count| assert_eq!(count.get(), 0));
    let bytes: i64 = reopened
        .connection
        .query_row(
            "SELECT octet_length(incarnation) FROM transcript_progress",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(usize::try_from(bytes).unwrap(), oversized.len());

    let (scope, records) = source_records(root.path()).await;
    let path = cache_path(root.path(), "suffix-identity.sqlite3");
    let mut first = cache(&path);
    first.apply(plan(&scope, 0, records[..2].to_vec())).unwrap();
    first
        .connection
        .execute(
            "UPDATE transcript_records SET record_id = ?1 WHERE position = ?2",
            params![&oversized, 1u64.to_be_bytes().as_slice()],
        )
        .unwrap();
    drop(first);
    let mut reopened = cache(&path);
    rows::IDENTIFIER_TEXT_READS.with(|count| count.set(0));
    assert_eq!(reopened.load(&scope), Err(StoreError::Failed));
    assert_eq!(reopened.take_refusal(), Some(CacheError::Corrupt));
    // Only the three bounded progress identifiers were acquired; no suffix ID.
    rows::IDENTIFIER_TEXT_READS.with(|count| assert_eq!(count.get(), 3));
    let bytes: i64 = reopened
        .connection
        .query_row(
            "SELECT octet_length(record_id) FROM transcript_records WHERE position = ?1",
            params![1u64.to_be_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(usize::try_from(bytes).unwrap(), oversized.len());
}

#[test]
fn changed_scope_requires_explicit_reset() {
    let root = tempfile::tempdir().unwrap();
    let mut cache = cache(&cache_path(root.path(), "cache.sqlite3"));
    cache.observe_head(&scope(), 0).unwrap();
    let foreign = Scope::new(
        id("receiver"),
        id("origin"),
        id("00000000-0000-0000-0000-000000000001"),
        id("other"),
        physical_record_schema(),
        id("epoch"),
    );
    assert_eq!(
        cache.load(&foreign),
        Err(StoreError::ScopeMismatch {
            saved: Box::new(scope()),
            requested: Box::new(foreign),
        })
    );
    let rows: i64 = cache
        .connection
        .query_row(
            "SELECT count(*) FROM transcript_progress WHERE incarnation = ?1",
            params!["incarnation"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
}

fn replacement(saved: &Scope) -> Scope {
    Scope::new(
        saved.receiver().clone(),
        saved.origin().clone(),
        saved.stream().clone(),
        saved.incarnation().clone(),
        saved.schema().clone(),
        id("next-epoch"),
    )
}
fn reset_request(saved: &Scope, generation: u64, operation: &str) -> CacheReset {
    CacheReset::new(
        id(operation),
        id("operator"),
        saved.clone(),
        generation,
        replacement(saved),
    )
    .unwrap()
}

#[tokio::test]
async fn reset_retry_returns_original_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_path(directory.path(), "reset.sqlite");
    let (saved, records) = source_records(directory.path()).await;
    let mut store = cache(&path);
    store.apply(plan(&saved, 0, records.clone())).unwrap();
    let before = store.cached_progress(&saved).unwrap().unwrap();
    let request = reset_request(&saved, before.generation, "reset-op");
    let receipt = store.reset(&request).unwrap();
    assert_eq!(receipt.request(), &request);
    assert_eq!(receipt.before(), &before);
    assert_eq!(receipt.after().downloaded, 0);
    assert_eq!(receipt.after().applied, 0);
    assert_eq!(receipt.after().facts, 0);
    assert_eq!(receipt.after().generation, before.generation + 1);
    assert_eq!(receipt.observed_at_ms(), 123_000);
    let audit = store.connection.query_row(
        "SELECT cause,initiator,caller,before_downloaded,before_applied,before_facts,after_generation,observed_at_ms FROM cache_resets WHERE operation=?1",
        params![request.operation().as_str()],
        |row| Ok(ResetEvidence {
            cause:row.get(0)?, initiator:row.get(1)?, caller:row.get(2)?,
            downloaded:row.get(3)?, applied:row.get(4)?, facts:row.get(5)?,
            generation:row.get(6)?, observed_at_ms:row.get(7)?,
        }),
    ).unwrap();
    assert_eq!(
        (
            audit.cause.as_str(),
            audit.initiator.as_str(),
            audit.caller.as_str()
        ),
        ("ExplicitReset", "LocalOperator", "operator")
    );
    assert_eq!(audit.downloaded, before.downloaded.to_be_bytes());
    assert_eq!(audit.applied, before.applied.to_be_bytes());
    assert_eq!(audit.facts, before.facts.to_be_bytes());
    assert_eq!(audit.generation, receipt.after().generation.to_be_bytes());
    assert_eq!(audit.observed_at_ms, receipt.observed_at_ms().to_be_bytes());

    assert_eq!(
        store
            .transcript(request.replacement())
            .unwrap()
            .view_state(),
        CommittedViewState::NotLoaded
    );
    let old = store.load(&saved).unwrap_err();
    assert!(matches!(old, StoreError::ScopeMismatch { .. }));
    store
        .apply(plan(
            request.replacement(),
            0,
            records
                .into_iter()
                .map(|record| Record {
                    scope: request.replacement().clone(),
                    ..record
                })
                .collect(),
        ))
        .unwrap();
    let after_work = store.cached_progress(request.replacement()).unwrap();
    drop(store);
    let mut reopened = cache(&path);
    assert_eq!(reopened.reset(&request).unwrap(), receipt);
    assert_eq!(
        reopened.cached_progress(request.replacement()).unwrap(),
        after_work
    );
    let count: i64 = reopened
        .connection
        .query_row("SELECT count(*) FROM cache_resets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn reset_rejects_conflicting_identity_and_stale_generation() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_path(directory.path(), "reset-conflict.sqlite");
    let saved = scope();
    let mut store = cache(&path);
    store.observe_head(&saved, 0).unwrap();
    let request = reset_request(&saved, 1, "reset-op");
    let mut competing = cache(&path);
    let wrong_generation = CacheReset::new(
        id("wrong-generation"),
        id("operator"),
        saved.clone(),
        2,
        saved.clone(),
    )
    .unwrap();
    assert_eq!(competing.reset(&wrong_generation), Err(CacheError::Stale));
    assert_eq!(
        competing
            .cached_progress(&saved)
            .unwrap()
            .unwrap()
            .generation,
        1
    );
    let receipts: i64 = competing
        .connection
        .query_row("SELECT count(*) FROM cache_resets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(receipts, 0);
    let receipt = store.reset(&request).unwrap();
    assert_eq!(
        competing.reset(&reset_request(&saved, 1, "delayed-op")),
        Err(CacheError::Stale)
    );
    let conflict = CacheReset::new(
        id("reset-op"),
        id("another-operator"),
        saved.clone(),
        1,
        replacement(&saved),
    )
    .unwrap();
    assert_eq!(
        competing.reset(&conflict),
        Err(CacheError::ConflictingRecord)
    );
    assert_eq!(
        competing.cached_progress(request.replacement()).unwrap(),
        Some(receipt.after().clone())
    );
    let missing = Scope::new(
        id("receiver"),
        id("origin"),
        id("absent"),
        id("incarnation"),
        physical_record_schema(),
        id("epoch"),
    );
    assert_eq!(
        store.reset(&reset_request(&missing, 1, "missing-op")),
        Err(CacheError::Stale)
    );
}

#[tokio::test]
async fn reset_audit_failure_rolls_back_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_path(directory.path(), "reset-failure.sqlite");
    let (saved, records) = source_records(directory.path()).await;
    let mut store = cache(&path);
    store.apply(plan(&saved, 0, records)).unwrap();
    let before = store.cached_progress(&saved).unwrap().unwrap();
    let checkpoint = store.transcript(&saved).unwrap().checkpoint().unwrap();
    store.connection.execute_batch("CREATE TRIGGER fail_reset_audit BEFORE INSERT ON cache_resets BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END;").unwrap();
    let request = reset_request(&saved, before.generation, "reset-op");
    assert_eq!(store.reset(&request), Err(CacheError::Unavailable));
    drop(store);
    let mut reopened = cache(&path);
    assert_eq!(reopened.cached_progress(&saved).unwrap(), Some(before));
    assert_eq!(
        reopened.transcript(&saved).unwrap().checkpoint().unwrap(),
        checkpoint
    );
    let count: i64 = reopened
        .connection
        .query_row("SELECT count(*) FROM cache_resets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    reopened
        .connection
        .execute_batch("DROP TRIGGER fail_reset_audit")
        .unwrap();
    assert!(reopened.reset(&request).is_ok());
}

#[test]
fn reset_is_atomic_and_preserves_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_path(directory.path(), "reset-fence.sqlite");
    let saved = scope();
    let mut store = cache(&path);
    store.observe_head(&saved, 0).unwrap();
    let catalogue = super::catalogue_tests::catalogue_scope();
    let pass = super::catalogue_tests::start(&mut store, 2);
    CatalogueStore::apply_page(
        &mut store,
        super::catalogue_tests::page(pass, vec![super::catalogue_tests::value(2, true)]),
    )
    .unwrap();
    assert_eq!(
        CatalogueStore::progress(&mut store, &catalogue)
            .unwrap()
            .unwrap()
            .completed,
        2
    );
    assert_eq!(
        store.reset(&reset_request(&saved, 1, "reset-op")),
        Err(CacheError::Fenced)
    );
    drop(store);
    let mut reopened = cache(&path);
    assert!(matches!(
        reopened.transcript(&saved),
        Err(CacheError::Fenced)
    ));
    let count: i64 = reopened
        .connection
        .query_row("SELECT count(*) FROM catalogue_deletions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(rows::progress(&reopened.connection, &saved).unwrap(), None);
}

#[tokio::test]
async fn reset_generation_refuses_delayed_page() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_path(directory.path(), "reset-generation.sqlite");
    let (saved, records) = source_records(directory.path()).await;
    let mut first = cache(&path);
    first.apply(plan(&saved, 0, records[..1].to_vec())).unwrap();
    let admitted = first.cached_progress(&saved).unwrap().unwrap();
    let mut delayed = cache(&path);
    delayed.load(&saved).unwrap();
    let old_plan = plan(&saved, 1, records[1..].to_vec());
    let request = CacheReset::new(
        id("reset-same-scope"),
        id("operator"),
        saved.clone(),
        admitted.generation,
        saved.clone(),
    )
    .unwrap();
    let receipt = first.reset(&request).unwrap();
    assert_eq!(delayed.apply(old_plan), Err(StoreError::Stale));
    assert_eq!(
        rows::progress(&delayed.connection, &saved).unwrap(),
        Some(receipt.after().clone())
    );
    delayed.load(&saved).unwrap();
    delayed.apply(plan(&saved, 0, records)).unwrap();
    assert!(delayed.cached_progress(&saved).unwrap().unwrap().downloaded > 0);
}

#[test]
fn unsupported_reset_schema_has_no_effects() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_path(directory.path(), "reset-schema.sqlite");
    let saved = scope();
    let mut store = cache(&path);
    store.observe_head(&saved, 0).unwrap();
    let prior = store.cached_progress(&saved).unwrap();
    let unsupported = Scope::new(
        saved.receiver().clone(),
        saved.origin().clone(),
        saved.stream().clone(),
        saved.incarnation().clone(),
        id("unsupported-schema"),
        saved.access_epoch().clone(),
    );
    let request = CacheReset::new(
        id("reset-op"),
        id("operator"),
        saved.clone(),
        1,
        unsupported,
    )
    .unwrap();
    assert!(matches!(
        store.reset(&request),
        Err(CacheError::TranscriptScope)
    ));
    assert_eq!(store.cached_progress(&saved).unwrap(), prior);
    let audit_count: i64 = store
        .connection
        .query_row("SELECT count(*) FROM cache_resets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(audit_count, 0);
}

#[test]
fn corrupt_reset_receipt_is_preserved() {
    for (column, invalid) in [
        ("before_applied", 1u64),
        ("before_facts", 99u64),
        ("after_generation", 99u64),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = cache_path(directory.path(), "reset-corrupt.sqlite");
        let saved = scope();
        let mut store = cache(&path);
        store.observe_head(&saved, 0).unwrap();
        let request = reset_request(&saved, 1, "reset-op");
        let original = store.reset(&request).unwrap();
        store
            .connection
            .execute(
                &format!("UPDATE cache_resets SET {column}=?1 WHERE operation=?2"),
                params![
                    invalid.to_be_bytes().as_slice(),
                    request.operation().as_str()
                ],
            )
            .unwrap();
        drop(store);
        let mut reopened = cache(&path);
        assert_eq!(reopened.reset(&request), Err(CacheError::Corrupt));
        assert_eq!(
            reopened.cached_progress(request.replacement()).unwrap(),
            Some(original.after().clone())
        );
        let retained: Vec<u8> = reopened
            .connection
            .query_row(
                &format!("SELECT {column} FROM cache_resets WHERE operation=?1"),
                params![request.operation().as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, invalid.to_be_bytes());
    }
}

// C23: independently valid-width counters can describe an impossible history.
// Refusal must precede reset's raw/checkpoint deletion and audit publication.
#[tokio::test]
async fn reset_refuses_impossible_progress_fact_count() {
    assert_impossible_reset_preserves_evidence(false).await;
}

#[tokio::test]
async fn reset_refuses_impossible_receipt_fact_count() {
    assert_impossible_reset_preserves_evidence(true).await;
}

async fn assert_impossible_reset_preserves_evidence(retry: bool) {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_path(directory.path(), "reset-facts.sqlite");
    let (saved, records) = source_records(directory.path()).await;
    let mut store = cache(&path);
    store.apply(plan(&saved, 0, records.clone())).unwrap();
    let before = store.cached_progress(&saved).unwrap().unwrap();
    assert!(before.facts < before.applied);
    let request = reset_request(&saved, before.generation, "reset-op");
    if retry {
        let receipt = store.reset(&request).unwrap();
        assert_eq!(receipt.before(), &before);
        store
            .apply(plan(
                request.replacement(),
                0,
                records
                    .into_iter()
                    .map(|record| Record {
                        scope: request.replacement().clone(),
                        ..record
                    })
                    .collect(),
            ))
            .unwrap();
    }
    let (table, column) = if retry {
        ("cache_resets", "before_facts")
    } else {
        ("transcript_progress", "facts")
    };
    store
        .connection
        .execute(
            &format!("UPDATE {table} SET {column}=?1"),
            params![(before.applied + 1).to_be_bytes().as_slice()],
        )
        .unwrap();
    drop(store);
    let mut reopened = cache(&path);
    let durable = std::fs::read(&path).unwrap();
    assert_eq!(
        reopened.reset(&request),
        Err(CacheError::Corrupt),
        "retry={retry}"
    );
    drop(reopened);
    assert_eq!(std::fs::read(&path).unwrap(), durable, "retry={retry}");
}

#[tokio::test]
async fn reset_accepts_aborted_physical_progress_and_retains_historical_retry() {
    use sha2::{Digest, Sha256};
    let directory = tempfile::tempdir().unwrap();
    let path = cache_path(directory.path(), "reset-abort.sqlite");
    let (saved, records) = source_records(directory.path()).await;
    // A real SDK-produced chunked start and piece followed by its exact abort
    // envelope reach the shared fold through the ordinary record cache surface.
    let start = &records[0];
    assert_eq!(start.payload[0], 1);
    let mut payload = vec![4, 1];
    payload.extend_from_slice(&1u64.to_be_bytes());
    payload.extend_from_slice(&2u64.to_be_bytes());
    payload.extend_from_slice(&Sha256::digest(&start.payload[1..]));
    let prefix = start.id.as_str().strip_suffix("-start").unwrap();
    let abort = Record {
        position: 3,
        id: id(&format!("{prefix}-abort-2")),
        scope: saved.clone(),
        payload,
    };
    let mut store = cache(&path);
    store
        .apply(plan(
            &saved,
            0,
            vec![records[0].clone(), records[1].clone(), abort],
        ))
        .unwrap();
    let before = store.cached_progress(&saved).unwrap().unwrap();
    assert_eq!((before.downloaded, before.applied, before.facts), (3, 0, 0));
    drop(store);
    let mut reopened = cache(&path);
    let request = reset_request(&saved, before.generation, "reset-aborted");
    let receipt = reopened.reset(&request).unwrap();
    assert_eq!(receipt.before(), &before);
    reopened
        .apply(plan(
            request.replacement(),
            0,
            records
                .into_iter()
                .map(|record| Record {
                    scope: request.replacement().clone(),
                    ..record
                })
                .collect(),
        ))
        .unwrap();
    let later = reopened.cached_progress(request.replacement()).unwrap();
    drop(reopened);
    let mut final_cache = cache(&path);
    assert_eq!(final_cache.reset(&request).unwrap(), receipt);
    assert_eq!(
        final_cache.cached_progress(request.replacement()).unwrap(),
        later
    );
}

#[test]
fn reset_commit_failure_is_unconfirmed_and_requires_reload() {
    let directory = tempfile::tempdir().unwrap();
    let path = cache_path(directory.path(), "reset-commit-failure.sqlite");
    let saved = scope();
    let mut store = cache(&path);
    store.observe_head(&saved, 0).unwrap();
    let before = store.cached_progress(&saved).unwrap();
    store.connection.execute_batch("CREATE TABLE audit_parent (id INTEGER PRIMARY KEY); CREATE TABLE audit_child (parent INTEGER REFERENCES audit_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER audit_commit_failure AFTER INSERT ON cache_resets BEGIN INSERT INTO audit_child VALUES (42); END;").unwrap();
    let request = reset_request(&saved, 1, "reset-op");
    assert_eq!(store.reset(&request), Err(CacheError::Uncertain));
    drop(store);
    let mut reopened = cache(&path);
    assert_eq!(reopened.cached_progress(&saved).unwrap(), before);
    let count: i64 = reopened
        .connection
        .query_row("SELECT count(*) FROM cache_resets", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    reopened
        .connection
        .execute_batch("DROP TRIGGER audit_commit_failure")
        .unwrap();
    assert!(reopened.reset(&request).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn borrowed_fold_restores_late_sql_failure_and_quota() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records_with_history(root.path()).await;
    let path = cache_path(root.path(), "borrowed.sqlite3");
    let mut store = cache(&path);
    // The first save ends at its actual completion2; the next Unit starts at3
    // and remains private until that outer save's final completion.
    store.apply(plan(&scope, 0, records[..3].to_vec())).unwrap();
    assert_eq!(store.transcript(&scope).unwrap().fact_count(), 1);
    assert!(store.transcript(&scope).unwrap().applied() < 3);
    store
        .observe_head(&scope, records.last().unwrap().position)
        .unwrap();
    let before = store.cached_progress(&scope).unwrap();
    let fold = store.transcript(&scope).unwrap();
    let status = fold.status();
    let positions = (fold.downloaded(), fold.applied(), fold.fact_count());
    let snapshot = fold.snapshot().unwrap().clone();
    let checkpoint = fold.checkpoint().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    for suffix in [records[3..4].to_vec(), records[3..].to_vec()] {
        store.connection.execute_batch("CREATE TRIGGER refuse_next_record BEFORE INSERT ON transcript_records BEGIN SELECT RAISE(ABORT, 'late write'); END;").unwrap();
        let durable = std::fs::read(&path).unwrap();
        assert_eq!(
            store.apply(plan(&scope, 3, suffix.clone())),
            Err(StoreError::Failed)
        );
        assert_eq!(store.take_refusal(), Some(CacheError::Unavailable));
        assert_eq!(store.cached_progress(&scope).unwrap(), before);
        let fold = store.transcript(&scope).unwrap();
        assert_eq!(fold.status(), status);
        assert_eq!(
            (fold.downloaded(), fold.applied(), fold.fact_count()),
            positions
        );
        assert_eq!(fold.snapshot(), Some(&snapshot));
        assert_eq!(fold.checkpoint().unwrap(), checkpoint);
        assert_eq!(std::fs::read(&path).unwrap(), durable);
        store
            .connection
            .execute_batch("DROP TRIGGER refuse_next_record")
            .unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap().len(), bytes.len());
    let original = store.policy;
    store.policy = CachePolicy::new(original.database_bytes(), 1, original.suffix_page()).unwrap();
    let durable = std::fs::read(&path).unwrap();
    assert_eq!(
        store.apply(plan(&scope, 3, records[3..].to_vec())),
        Err(StoreError::Failed)
    );
    assert_eq!(store.take_refusal(), Some(CacheError::Quota));
    let fold = store.transcript(&scope).unwrap();
    assert_eq!(fold.status(), status);
    assert_eq!(
        (fold.downloaded(), fold.applied(), fold.fact_count()),
        positions
    );
    assert_eq!(fold.snapshot(), Some(&snapshot));
    assert_eq!(fold.checkpoint().unwrap(), checkpoint);
    assert_eq!(store.cached_progress(&scope).unwrap(), before);
    assert_eq!(std::fs::read(&path).unwrap(), durable);
    drop(store);
    let mut reopened = cache(&path);
    assert_eq!(reopened.cached_progress(&scope).unwrap(), before);
    assert_eq!(
        reopened.transcript(&scope).unwrap().checkpoint().unwrap(),
        checkpoint
    );
    reopened
        .apply(plan(&scope, 3, records[3..].to_vec()))
        .unwrap();
    assert_eq!(reopened.transcript(&scope).unwrap().fact_count(), 2);
}

#[test]
fn borrowed_head_restores_late_sql_failure_and_quota() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "head-rollback.sqlite3");
    let mut store = cache(&path);
    let before = store.transcript(&scope()).unwrap().checkpoint().unwrap();
    let status = store.transcript(&scope()).unwrap().status();
    store.connection.execute_batch("CREATE TRIGGER refuse_checkpoint BEFORE INSERT ON transcript_checkpoints BEGIN SELECT RAISE(ABORT, 'late checkpoint'); END;").unwrap();
    let durable = std::fs::read(&path).unwrap();
    assert_eq!(
        store.observe_head(&scope(), 0),
        Err(CacheError::Unavailable)
    );
    assert_eq!(store.cached_progress(&scope()).unwrap(), None);
    assert_eq!(store.transcript(&scope()).unwrap().status(), status);
    assert_eq!(
        store.transcript(&scope()).unwrap().checkpoint().unwrap(),
        before
    );
    assert_eq!(std::fs::read(&path).unwrap(), durable);
    store
        .connection
        .execute_batch("DROP TRIGGER refuse_checkpoint")
        .unwrap();
    let original = store.policy;
    store.policy = CachePolicy::new(original.database_bytes(), 1, original.suffix_page()).unwrap();
    let durable = std::fs::read(&path).unwrap();
    assert_eq!(store.observe_head(&scope(), 0), Err(CacheError::Quota));
    assert_eq!(store.cached_progress(&scope()).unwrap(), None);
    assert_eq!(store.transcript(&scope()).unwrap().status(), status);
    assert_eq!(
        store.transcript(&scope()).unwrap().checkpoint().unwrap(),
        before
    );
    assert_eq!(std::fs::read(&path).unwrap(), durable);
    drop(store);
    let mut reopened = cache(&path);
    assert_eq!(
        reopened.transcript(&scope()).unwrap().checkpoint().unwrap(),
        before
    );
    reopened.observe_head(&scope(), 0).unwrap();
    assert_eq!(
        reopened.transcript(&scope()).unwrap().view_state(),
        CommittedViewState::CompleteEmpty
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hot_cache_page_does_not_clone_pending_prefix() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_long_pending_records(root.path()).await;
    assert!(records.len() > 22);
    let mut store = cache(&cache_path(root.path(), "allocation.sqlite3"));
    for page in records[..20].chunks(1) {
        let after = page[0].position - 1;
        store.apply(plan(&scope, after, page.to_vec())).unwrap();
    }
    let fold = store.transcript(&scope).unwrap();
    assert_eq!(fold.fact_count(), 1);
    let snapshot = fold.snapshot().unwrap().provider.context().as_ptr();
    let prefix = records[2..20]
        .iter()
        .map(|record| record.payload.len())
        .sum::<usize>();
    let next = plan(&scope, 20, records[20..21].to_vec());
    let (result, allocated) = super::allocations::measure(|| store.apply(next));
    result.unwrap();
    eprintln!("hot cache page: {allocated} Rust allocation bytes; {prefix} retained physical prefix bytes");
    assert!(
        allocated < prefix / 2,
        "hot page allocated {allocated} bytes against {prefix} prior physical bytes"
    );
    assert_eq!(
        store
            .transcript(&scope)
            .unwrap()
            .snapshot()
            .unwrap()
            .provider
            .context()
            .as_ptr(),
        snapshot
    );
    assert_eq!(store.transcript(&scope).unwrap().downloaded(), 21);
}

fn retarget_record(record: &Record, position: u64) -> Record {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    let mut payload = record.payload.clone();
    assert_eq!(payload[0], 1, "inline start frame");
    let id_len = u16::from_be_bytes([payload[3], payload[4]]) as usize;
    let attempt_at = 13 + id_len;
    payload[attempt_at..attempt_at + 8].copy_from_slice(&position.to_be_bytes());
    let mut identity = payload[2..attempt_at].to_vec();
    identity.extend_from_slice(&position.to_be_bytes());
    let digest = Sha256::digest(&identity);
    let mut event_id = String::from("nessa-fact-");
    for byte in digest {
        write!(event_id, "{byte:02x}").unwrap();
    }
    event_id.push_str("-start");
    Record {
        position,
        id: id(&event_id),
        scope: record.scope.clone(),
        payload,
    }
}

fn another_version_record(record: &Record) -> Record {
    use sha2::{Digest, Sha256};
    let mut payload = record.payload.clone();
    assert_eq!(payload.first().copied(), Some(1), "start frame");
    let id_len = u16::from_be_bytes([payload[3], payload[4]]) as usize;
    let attempt_at = 13 + id_len;
    let body_len =
        u64::from_be_bytes(payload[attempt_at + 8..attempt_at + 16].try_into().unwrap()) as usize;
    let digest_at = attempt_at + 20;
    let mode_at = digest_at + 32;
    assert_eq!(payload[mode_at], 0, "inline frame");
    let body_at = mode_at + 1;
    assert_eq!(
        body_at + body_len,
        payload.len(),
        "inline body fills the frame"
    );
    // The fact body is a 136-byte save header followed by the semantic batch.
    // A completion envelope has no batch. Leaving it intact is enough: the
    // unit ahead of it is the gap, and this frame stays physically valid.
    const HEADER_BYTES: usize = 136;
    let json_at = body_at + HEADER_BYTES;
    let marker = b"{\"schemaVersion\":1";
    if payload.len() < json_at + marker.len() || payload[json_at..json_at + marker.len()] != *marker
    {
        return Record {
            payload,
            ..record.clone()
        };
    }
    payload[json_at + marker.len() - 1] = b'2';
    let payload_hash = Sha256::digest(&payload[json_at..]);
    payload[body_at + 104..body_at + HEADER_BYTES].copy_from_slice(&payload_hash);
    let frame_hash = Sha256::digest(&payload[body_at..]);
    payload[digest_at..digest_at + 32].copy_from_slice(&frame_hash);
    Record {
        payload,
        ..record.clone()
    }
}

/// A gap's unread records are not kept. Restore does not re-read them, a
/// later continuation is, and further gap pages do not grow the table.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_restore_after_a_gap_drops_the_unread_tail() {
    use nessa_sdk::application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage},
    };
    use nessa_sdk::domain::agent_execution::sessions::{
        ExecutionSessionId, ProviderContext, SessionId,
    };
    use nessa_sdk::infrastructure::session_storage::{RecordReadStatus, RecordStorage};
    use nessa_sync::replication::domain::PageRequest;

    async fn read_records(storage: &RecordStorage, session: &SessionId) -> (Scope, Vec<Record>) {
        let mut source = storage
            .record_source(session, id("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(id("receiver"), id("epoch"));
        let returned = tokio::task::spawn_blocking(move || {
            let target = loop {
                if let RecordReadStatus::Ready(head) = source.bounded_head(&scope).unwrap() {
                    break head;
                }
            };
            let mut records = vec![];
            let mut after = 0;
            while after < target {
                let limits = policy().suffix_page();
                let request = PageRequest {
                    scope: scope.clone(),
                    after,
                    target,
                    max_records: limits.max_records(),
                    max_payload_bytes: limits.max_payload_bytes(),
                    max_record_bytes: limits.max_record_bytes(),
                };
                let RecordReadStatus::Ready(page) = source.bounded_page(&request).unwrap() else {
                    panic!("head validated the captured prefix");
                };
                after = page.records.last().unwrap().position;
                records.extend(page.records);
            }
            drop(source);
            (scope, records)
        })
        .await
        .unwrap();
        returned
    }

    let root = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(root.path().join("source")).unwrap();
    storage.initialize().await.unwrap();
    let session = SessionId::new("00000000-0000-0000-0000-000000000001").unwrap();
    let provider = ProviderIdentity::new("provider", "model", "workspace").unwrap();
    let lease = storage.open(session.clone()).await.unwrap();
    let opened = SessionSnapshot {
        id: session.clone(),
        provider: provider.clone(),
        provider_context: ProviderContext::Absent,
        invocations: vec![],
        queue_history: vec![],
    };
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            opened,
            vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                id: session.clone(),
                provider: provider.clone(),
                context: ProviderContext::Absent,
            }])
            .unwrap()],
        )
        .await
        .unwrap();
    drop(lease);
    let (scope, prefix) = read_records(&storage, &session).await;
    let lease = storage.open(session.clone()).await.unwrap();
    let kept = ProviderContext::Recorded(ExecutionSessionId::new("kept-after-gap").unwrap());
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            SessionSnapshot {
                id: session.clone(),
                provider: provider.clone(),
                provider_context: kept.clone(),
                invocations: vec![],
                queue_history: vec![],
            },
            vec![SessionSaveUnit::new(vec![SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: kept.clone(),
            }])
            .unwrap()],
        )
        .await
        .unwrap();
    drop(lease);
    let (_, all) = read_records(&storage, &session).await;
    storage.shutdown().await.unwrap();
    let suffix = &all[prefix.len()..];
    assert!(!suffix.is_empty());
    let mut gap = suffix
        .iter()
        .map(another_version_record)
        .collect::<Vec<_>>();
    let mut cursor = gap.last().unwrap().position;
    for _ in 0..3 {
        for record in suffix {
            cursor += 1;
            gap.push(another_version_record(&retarget_record(record, cursor)));
        }
    }
    let mut continuation = Vec::new();
    for record in suffix {
        cursor += 1;
        continuation.push(retarget_record(record, cursor));
    }
    let path = cache_path(root.path(), "gap.sqlite3");
    let mut store = cache(&path);
    let mut page = prefix.clone();
    page.extend(gap);
    let mut after = 0u64;
    for chunk in page.chunks(8) {
        store.apply(plan(&scope, after, chunk.to_vec())).unwrap();
        after = chunk.last().unwrap().position;
    }
    let kept_rows = table_rows(&store, "transcript_records");
    assert_eq!(kept_rows, prefix.len() as i64);
    assert!(store.transcript(&scope).unwrap().snapshot().is_some());
    assert!(matches!(
        store
            .transcript(&scope)
            .unwrap()
            .snapshot()
            .unwrap()
            .provider_context,
        ProviderContext::Absent
    ));
    assert_eq!(store.transcript(&scope).unwrap().downloaded(), after);
    let progress = store.cached_progress(&scope).unwrap();
    drop(store);

    let mut restored = cache(&path);
    assert_eq!(restored.cached_progress(&scope).unwrap(), progress);
    assert_eq!(table_rows(&restored, "transcript_records"), kept_rows);
    assert_eq!(restored.transcript(&scope).unwrap().downloaded(), after);
    assert!(matches!(
        restored
            .transcript(&scope)
            .unwrap()
            .snapshot()
            .unwrap()
            .provider_context,
        ProviderContext::Absent
    ));
    restored
        .apply(plan(&scope, after, continuation.clone()))
        .unwrap();
    match &restored
        .transcript(&scope)
        .unwrap()
        .snapshot()
        .unwrap()
        .provider_context
    {
        ProviderContext::Recorded(session) => assert_eq!(session.as_str(), "kept-after-gap"),
        other => panic!("continuation missing from the restored view: {other:?}"),
    }
    assert_eq!(
        table_rows(&restored, "transcript_records"),
        kept_rows + continuation.len() as i64
    );
    let retry = plan(&scope, 0, page);
    assert_eq!(restored.apply(retry), Ok(()));
    assert_eq!(
        table_rows(&restored, "transcript_records"),
        kept_rows + continuation.len() as i64
    );
}
