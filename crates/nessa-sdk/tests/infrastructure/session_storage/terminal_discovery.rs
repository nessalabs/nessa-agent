//! Real SQLite bounded discovery, lifetime, refusal and work-accounting evidence.

use super::*;
use crate::application::agent_execution::sessions::SessionStorage;
use crate::{
    application::agent_execution::sessions::records::{FactKey, FactKind},
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::RecordStorage,
};
use event_stream::{EventSink, StreamId};
use nessa_sync::replication::domain::{Id, PageRequest};
use std::sync::atomic::Ordering;

fn sid(value: &str) -> Id {
    Id::new(value).unwrap()
}
async fn head(storage: &RecordStorage, id: &SessionId) -> RecordReadStatus<u64> {
    let mut source = storage
        .record_source(id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || source.bounded_head(&scope).unwrap())
            .join()
            .unwrap()
    })
    .await
    .unwrap()
}
async fn append_fact(
    storage: &RecordStorage,
    stream: &StreamKey,
    ordinal: u64,
    bytes: usize,
) -> u64 {
    let runtime = storage.runtime().await.unwrap();
    let start = runtime.bounds(stream).await.unwrap().tail.offset + 1;
    let fact = super::super::stream_fact::FramedFact {
        key: FactKey::new(
            if ordinal == 0 {
                FactKind::SessionOpen
            } else {
                FactKind::ProviderContext
            },
            None,
            ordinal,
        )
        .unwrap(),
        body: vec![b'x'; bytes],
    };
    let frames = super::super::stream_fact::frame_fact(&fact, start).unwrap();
    for frame in frames {
        runtime.append(stream, frame).await.unwrap();
    }
    runtime.bounds(stream).await.unwrap().tail.offset
}

#[tokio::test]
async fn recreated_sources_resume_bounded_large_fact_validation_and_pages_do_not_rescan() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let runtime = storage.runtime().await.unwrap();
    let stream = runtime
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    let target = append_fact(&storage, &stream, 0, 2 * 1024 * 1024).await;
    assert!(target > 16);
    let mut calls = 0;
    loop {
        calls += 1;
        let before = storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst);
        let before_bytes = storage.terminal_cache.returned_bytes.load(Ordering::SeqCst);
        let status = head(&storage, &id).await;
        let bytes = storage.terminal_cache.returned_bytes.load(Ordering::SeqCst) - before_bytes;
        assert!(bytes <= 1024 * 1024);
        let after = storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst);
        assert!(after - before <= 16);
        if status == RecordReadStatus::Ready(target) {
            break;
        }
        assert_eq!(status, RecordReadStatus::Preparing);
        assert!(calls < 10);
    }
    assert!(calls > 1);
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        target as usize
    );
    let mut source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    let read = tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || {
            let mut after = 0;
            while after < target {
                let request = PageRequest {
                    scope: scope.clone(),
                    after,
                    target,
                    max_records: 1,
                    max_payload_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                    max_record_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                };
                let RecordReadStatus::Ready(page) = source.bounded_page(&request).unwrap() else {
                    panic!("validated prefix needs no preparation")
                };
                after = page.records.last().unwrap().position;
            }
            drop(source);
        })
        .join()
        .unwrap()
    })
    .await;
    read.unwrap();
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        target as usize,
        "paging does not validate the prefix again"
    );
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn long_inline_history_enforces_frame_step_limit_and_unchanged_head_does_no_read_work() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let stream = storage
        .runtime()
        .await
        .unwrap()
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    let mut target = 0;
    for index in 0..129 {
        target = append_fact(&storage, &stream, index, 8).await;
    }
    loop {
        let before = storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst);
        let status = head(&storage, &id).await;
        let after = storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst);
        assert!(
            after - before <= 16,
            "physical frame work must obey its bound even when records are tiny"
        );
        if status == RecordReadStatus::Ready(target) {
            break;
        }
    }
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        129
    );
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(target));
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        129
    );
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn discovery_finishes_captured_tail_under_new_writes_then_discovers_later_head() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let stream = storage
        .runtime()
        .await
        .unwrap()
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    let first = append_fact(&storage, &stream, 0, 2 * 1024 * 1024).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
    let later = append_fact(&storage, &stream, 1, 8).await;
    loop {
        match head(&storage, &id).await {
            RecordReadStatus::Preparing => {}
            RecordReadStatus::Ready(value) => {
                assert_eq!(value, first);
                break;
            }
        }
    }
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(later));
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        later as usize
    );
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn partial_tail_preserves_hash_until_seal_and_unknown_target_stays_invalid() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let runtime = storage.runtime().await.unwrap();
    let stream = runtime
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    let fact = super::super::stream_fact::FramedFact {
        key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
        body: vec![b'x'; 2 * 1024 * 1024],
    };
    let frames = super::super::stream_fact::frame_fact(&fact, 1).unwrap();
    for frame in frames.iter().take(frames.len() - 1) {
        runtime.append(&stream, frame.clone()).await.unwrap();
    }
    loop {
        match head(&storage, &id).await {
            RecordReadStatus::Preparing => {}
            RecordReadStatus::Ready(value) => {
                assert_eq!(value, 0);
                break;
            }
        }
    }
    let mut source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let request = PageRequest {
        scope: source.scope(sid("receiver"), sid("epoch")),
        after: 0,
        target: 2,
        max_records: 1,
        max_payload_bytes: 128,
        max_record_bytes: 128,
    };
    let result = tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || source.bounded_page(&request))
            .join()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(result, Err(SourceError::InvalidRequest));
    runtime
        .append(&stream, frames.last().unwrap().clone())
        .await
        .unwrap();
    assert_eq!(
        head(&storage, &id).await,
        RecordReadStatus::Ready(frames.len() as u64)
    );
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        frames.len()
    );
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn abandoned_answer_retains_progress_and_cold_cache_repeats_only_bounded_steps() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let runtime = storage.runtime().await.unwrap();
    let stream = runtime
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    let target = append_fact(&storage, &stream, 0, 2 * 1024 * 1024).await;
    let source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    source.abandon_bounded_head_answer(scope);
    // Final drop on a non-entered thread joins admitted physical work even
    // though its reply consumer has gone away.
    tokio::task::spawn_blocking(move || std::thread::spawn(move || drop(source)).join().unwrap())
        .await
        .unwrap();
    let first = storage
        .terminal_cache
        .returned_records
        .load(Ordering::SeqCst);
    assert!(first > 0 && first <= 16);
    loop {
        if head(&storage, &id).await == RecordReadStatus::Ready(target) {
            break;
        }
    }
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        target as usize
    );
    storage.terminal_cache.entries.lock().unwrap().clear();
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
    let after = storage
        .terminal_cache
        .returned_records
        .load(Ordering::SeqCst);
    assert!(after - target as usize <= 16);
    loop {
        if head(&storage, &id).await == RecordReadStatus::Ready(target) {
            break;
        }
    }
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        2 * target as usize
    );
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn occupied_stream_and_full_active_cache_refuse_without_replacement_work() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let runtime = storage.runtime().await.unwrap();
    let stream = runtime
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    append_fact(&storage, &stream, 0, 8).await;
    let owner = storage.terminal_cache.acquire(&stream).unwrap();
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
    let mut occupied = vec![owner];
    for index in 1..CACHE_ENTRIES {
        occupied.push(
            storage
                .terminal_cache
                .acquire(&StreamKey {
                    id: StreamId::new(format!("occupied-{index}")).unwrap(),
                    incarnation: event_stream::IncarnationId([0; 16]),
                })
                .unwrap(),
        );
    }
    let another = SessionId::new("another").unwrap();
    let second = runtime
        .create_stream(&StreamId::new(another.as_str()).unwrap())
        .await
        .unwrap();
    append_fact(&storage, &second, 0, 8).await;
    assert_eq!(head(&storage, &another).await, RecordReadStatus::Preparing);
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(storage.terminal_cache.entries.lock().unwrap().len(), 16);
    drop(occupied);
    assert_eq!(head(&storage, &another).await, RecordReadStatus::Ready(1));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_scope_and_page_input_do_not_check_out_or_publish_progress() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let runtime = storage.runtime().await.unwrap();
    let stream = runtime
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    append_fact(&storage, &stream, 0, 8).await;
    let mut source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    let wrong = nessa_sync::replication::domain::Scope::new(
        scope.receiver().clone(),
        sid("foreign"),
        scope.stream().clone(),
        scope.incarnation().clone(),
        scope.schema().clone(),
        scope.access_epoch().clone(),
    );
    let request = PageRequest {
        scope,
        after: 0,
        target: 1,
        max_records: 0,
        max_payload_bytes: 128,
        max_record_bytes: 128,
    };
    let failures = tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || (source.bounded_head(&wrong), source.bounded_page(&request)))
            .join()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(
        failures,
        (
            Err(SourceError::IdentityChanged),
            Err(SourceError::InvalidRequest)
        )
    );
    assert!(storage.terminal_cache.entries.lock().unwrap().is_empty());
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(1));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn cached_old_incarnation_and_pruned_prefix_are_typed_refusals() {
    use event_stream::{
        AdvanceRetentionFloor, EnableRetryPolicy, LifecycleAction, LifecycleOperationId,
        LifecycleRequest, RetentionOperationId,
    };
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let runtime = storage.runtime().await.unwrap();
    let stream = runtime
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    append_fact(&storage, &stream, 0, 2 * 1024 * 1024).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
    let mut old = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let old_scope = old.scope(sid("receiver"), sid("epoch"));
    runtime
        .change_lifecycle(LifecycleRequest {
            operation_id: LifecycleOperationId::new("reset").unwrap(),
            expected: stream,
            action: LifecycleAction::Reset,
        })
        .await
        .unwrap();
    let refusal = tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || old.bounded_head(&old_scope))
            .join()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(refusal, Err(SourceError::IdentityChanged));
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(0));
    let replacement = runtime
        .find_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap()
        .unwrap();
    append_fact(&storage, &replacement, 0, 8).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(1));
    runtime
        .enable_retry_policy(EnableRetryPolicy {
            operation_id: RetentionOperationId::new("enable-prune").unwrap(),
            stream: replacement.clone(),
        })
        .await
        .unwrap();
    runtime
        .advance_retention_floor(AdvanceRetentionFloor {
            operation_id: RetentionOperationId::new("prune").unwrap(),
            stream: replacement.clone(),
            expected_floor: Cursor::new(replacement.clone(), 0),
            new_floor: Cursor::new(replacement, 1),
        })
        .await
        .unwrap();
    let mut source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    let refusal = tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || source.bounded_head(&scope))
            .join()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(refusal, Err(SourceError::Pruned));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn cached_partial_prefix_accepts_abort_then_later_fact_and_refuses_corrupt_tail() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let runtime = storage.runtime().await.unwrap();
    let stream = runtime
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    let fact = super::super::stream_fact::FramedFact {
        key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
        body: vec![b'x'; 2 * 1024 * 1024],
    };
    let frames = super::super::stream_fact::frame_fact(&fact, 1).unwrap();
    for frame in frames.iter().take(20) {
        runtime.append(&stream, frame.clone()).await.unwrap();
    }
    loop {
        if let RecordReadStatus::Ready(value) = head(&storage, &id).await {
            assert_eq!(value, 0);
            break;
        }
    }
    let aborted = super::super::stream_fact::abort_partial_fact(
        runtime,
        &stream,
        &Cursor::new(stream.clone(), 0),
    )
    .await
    .unwrap()
    .offset;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(aborted));
    let valid = append_fact(&storage, &stream, 1, 8).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(valid));
    let mut invalid = super::super::stream_fact::frame_fact(
        &super::super::stream_fact::FramedFact {
            key: FactKey::new(FactKind::ProviderContext, None, 2).unwrap(),
            body: vec![b'x'; 8],
        },
        valid + 1,
    )
    .unwrap()
    .remove(0);
    invalid.payload = event_stream::Payload::copy_from_slice(&[99]);
    runtime.append(&stream, invalid).await.unwrap();
    let mut source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    let refusal = tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || source.bounded_head(&scope))
            .join()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(refusal, Err(SourceError::Unavailable));
    {
        let cache = storage.terminal_cache.entries.lock().unwrap();
        let saved = cache
            .iter()
            .find(|entry| entry.key == stream)
            .unwrap()
            .state
            .as_ref()
            .unwrap();
        assert_eq!(saved.terminal, valid);
        assert_eq!(
            saved.validator.offset(),
            valid,
            "invalid framing does not publish progress"
        );
    }
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn maximum_accounted_corrupt_record_is_bounded_and_publishes_no_progress() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let runtime = storage.runtime().await.unwrap();
    let stream = runtime
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    let mut invalid = super::super::stream_fact::frame_fact(
        &super::super::stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body: vec![b'x'; 8],
        },
        1,
    )
    .unwrap()
    .remove(0);
    invalid.payload = event_stream::Payload::copy_from_slice(&[]);
    let payload = vec![99; 1024 * 1024 - invalid.accounted_bytes()];
    invalid.payload = event_stream::Payload::copy_from_slice(&payload);
    assert_eq!(invalid.accounted_bytes(), 1024 * 1024);
    runtime.append(&stream, invalid).await.unwrap();
    let mut source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    let refusal = tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || source.bounded_head(&scope))
            .join()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(refusal, Err(SourceError::Unavailable));
    assert_eq!(
        storage.terminal_cache.returned_bytes.load(Ordering::SeqCst),
        1024 * 1024
    );
    {
        let cache = storage.terminal_cache.entries.lock().unwrap();
        let saved = cache.front().unwrap().state.as_ref().unwrap();
        assert_eq!((saved.validator.offset(), saved.terminal), (0, 0));
    }
    storage.shutdown().await.unwrap();
}

#[test]
fn cold_restart_child() {
    let Ok(root) = std::env::var("NESSA_315_RESTART_ROOT") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let storage = RecordStorage::new(root).unwrap();
        let id = SessionId::new("conversation").unwrap();
        assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
        let mut calls = 1;
        let terminal = loop {
            calls += 1;
            if let RecordReadStatus::Ready(head) = head(&storage, &id).await {
                break head;
            }
            assert!(calls < 10)
        };
        println!("NESSA_315_RESTART {terminal}");
        assert_eq!(
            storage
                .terminal_cache
                .returned_records
                .load(Ordering::SeqCst),
            terminal as usize
        );
        storage.shutdown().await.unwrap();
    });
}

#[tokio::test]
async fn restarted_process_revalidates_durable_stream_in_bounded_steps() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let stream = storage
        .runtime()
        .await
        .unwrap()
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    let target = append_fact(&storage, &stream, 0, 2 * 1024 * 1024).await;
    while head(&storage, &id).await != RecordReadStatus::Ready(target) {}
    storage.shutdown().await.unwrap();
    drop(storage);
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "infrastructure::session_storage::terminal_discovery::tests::cold_restart_child",
                "--nocapture",
            ])
            .env("NESSA_315_RESTART_ROOT", root)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&format!("NESSA_315_RESTART {target}"))
    );
}

#[test]
fn cache_bounds_entries_and_exclusive_owner_returns_progress_after_unwind() {
    let cache = Arc::new(TerminalCache::default());
    let key = |index| StreamKey {
        id: StreamId::new(format!("stream-{index}")).unwrap(),
        incarnation: event_stream::IncarnationId([0; 16]),
    };
    let first = key(0);
    let owner = cache.acquire(&first).unwrap();
    assert!(cache.acquire(&first).is_none());
    for index in 1..CACHE_ENTRIES {
        drop(cache.acquire(&key(index)).unwrap())
    }
    assert_eq!(cache.entries.lock().unwrap().len(), CACHE_ENTRIES);
    drop(cache.acquire(&key(CACHE_ENTRIES)).unwrap());
    assert!(
        cache
            .entries
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry.key == first),
        "active owner cannot be evicted"
    );
    let result = std::panic::catch_unwind(move || {
        let _owner = owner;
        panic!("physical operation failed")
    });
    assert!(result.is_err());
    assert!(cache.acquire(&first).is_some());
    assert_eq!(cache.entries.lock().unwrap().len(), CACHE_ENTRIES);
}

#[tokio::test]
async fn byte_limited_lookahead_returns_only_prefix_then_validates_or_refuses() {
    for malformed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("records")).unwrap();
        let id = SessionId::new("conversation").unwrap();
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap();
        // Sixteen candidate frames fit the count cap, but fifteen pieces plus
        // their start consume the byte cap before the next piece can return.
        let fact = super::super::stream_fact::FramedFact {
            key: FactKey::new(FactKind::SessionOpen, None, 0).unwrap(),
            body: vec![b'x'; 2 * 1024 * 1024],
        };
        let mut frames = super::super::stream_fact::frame_fact(&fact, 1).unwrap();
        let first_count = frames[16..]
            .iter()
            .scan(0usize, |bytes, frame| {
                *bytes += frame.accounted_bytes();
                Some(*bytes)
            })
            .take_while(|bytes| *bytes <= super::super::MAX_STORED_RECORD_BYTES)
            .count();
        assert!(first_count < 16);
        let lookahead = frames[16 + first_count].accounted_bytes();
        let returned: usize = frames[16..16 + first_count]
            .iter()
            .map(|frame| frame.accounted_bytes())
            .sum();
        assert!(returned + lookahead <= 2 * super::super::MAX_STORED_RECORD_BYTES);
        if malformed {
            frames[16 + first_count].payload = event_stream::Payload::copy_from_slice(&[99]);
            // Preserve the original large charge so this malformed frame remains
            // a lookahead candidate instead of fitting the first returned page.
            frames[16 + first_count].payload = event_stream::Payload::copy_from_slice(&vec![
                99;
                lookahead
                    - (frames[16 + first_count].accounted_bytes()
                        - 1)
            ]);
        }
        for frame in frames {
            runtime.append(&stream, frame).await.unwrap();
        }
        assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
        let before_records = storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst);
        let before_bytes = storage.terminal_cache.returned_bytes.load(Ordering::SeqCst);
        assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
        assert_eq!(
            storage
                .terminal_cache
                .returned_records
                .load(Ordering::SeqCst)
                - before_records,
            first_count
        );
        assert_eq!(
            storage.terminal_cache.returned_bytes.load(Ordering::SeqCst) - before_bytes,
            returned
        );
        let mut source = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(sid("receiver"), sid("epoch"));
        let result = tokio::task::spawn_blocking(move || {
            std::thread::spawn(move || source.bounded_head(&scope))
                .join()
                .unwrap()
        })
        .await
        .unwrap();
        if malformed {
            assert_eq!(result, Err(SourceError::Unavailable));
            let cache = storage.terminal_cache.entries.lock().unwrap();
            let progress = cache.front().unwrap().state.as_ref().unwrap();
            assert_eq!(progress.validator.offset(), (16 + first_count) as u64);
            assert_eq!(progress.terminal, 0);
        } else {
            assert!(matches!(result, Ok(RecordReadStatus::Ready(_))));
        }
        storage.shutdown().await.unwrap();
    }
}
