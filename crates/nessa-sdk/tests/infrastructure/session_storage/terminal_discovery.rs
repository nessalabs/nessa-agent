//! Real SQLite bounded discovery, lifetime, refusal and work-accounting evidence.

use super::super::{
    save_group::{Header, SaveIdentity, EMPTY_CHAIN},
    stream_fact::{self, FramedFact},
};
use super::*;
use crate::application::agent_execution::sessions::SessionStorage;
use crate::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{
            records::{self, FactKey, FactKind},
            SessionChange, SessionSaveBackend, SessionSaveGeneration, SessionSaveUnit,
            SessionSnapshot,
        },
    },
    domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext, SessionId},
    infrastructure::session_storage::RecordStorage,
};
use event_stream::{
    AdvanceRetentionFloor, EnableRetryPolicy, EventSink, IncarnationId, LifecycleAction,
    LifecycleOperationId, LifecycleRequest, NewEvent, Payload, RetentionOperationId, StreamId,
};
use nessa_sync::replication::application::RecordSource;
use nessa_sync::replication::domain::{Id, Page, PageRequest, Scope};
use rusqlite::Connection;
use std::{env, panic, process::Command, sync::atomic::Ordering, thread};

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
        thread::spawn(move || source.bounded_head(&scope).unwrap())
            .join()
            .unwrap()
    })
    .await
    .unwrap()
}
// These discovery fixtures exercise the actual save envelope and typed payload
// syntax. Canonical semantic application is separately checked by writer/fold tests.
fn save_facts(stream: &StreamKey, base: u64, generation: u64, bytes: usize) -> [FramedFact; 2] {
    let change = if generation == 0 {
        SessionChange::Opened {
            id: SessionId::new(stream.id.as_str()).unwrap(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        }
    } else {
        SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Absent,
        }
    };
    let mut payload = super::super::snapshot::encode_semantic_batch(&[change]).unwrap();
    // JSON trailing whitespace preserves the typed unit while controlling the
    // physical piece budget without inventing an unsupported semantic codec.
    payload.resize(payload.len().max(bytes), b' ');
    let binding = SessionSaveGeneration::new(
        SessionSaveBackend::Record {
            stream: sid(stream.id.as_str()),
            incarnation: stream.incarnation.0,
        },
        base,
        generation,
    );
    let identity = SaveIdentity::binding(&binding).unwrap();
    let unit = Header::unit(identity.clone(), 0, EMPTY_CHAIN, &payload);
    let complete = Header::unit(identity, 1, unit.chain(payload.len() as u64), &[]);
    [
        FramedFact {
            key: FactKey::new(FactKind::SaveUnit, None, 0).unwrap(),
            body: unit.encode(&payload),
        },
        FramedFact {
            key: FactKey::new(FactKind::SaveComplete, None, 1).unwrap(),
            body: complete.encode(&[]),
        },
    ]
}
fn save_frames(
    stream: &StreamKey,
    base: u64,
    generation: u64,
    bytes: usize,
    start: u64,
) -> Vec<NewEvent> {
    let mut frames = Vec::new();
    for fact in save_facts(stream, base, generation, bytes) {
        frames.extend(stream_fact::frame_fact(&fact, start + frames.len() as u64).unwrap());
    }
    frames
}
async fn append_save(
    storage: &RecordStorage,
    stream: &StreamKey,
    generation: u64,
    bytes: usize,
) -> u64 {
    let runtime = storage.runtime().await.unwrap();
    let base = runtime.bounds(stream).await.unwrap().tail.offset;
    for frame in save_frames(stream, base, generation, bytes, base + 1) {
        runtime.append(stream, frame).await.unwrap();
    }
    runtime.bounds(stream).await.unwrap().tail.offset
}

// Unlike raw envelope fixtures, these saves pass the canonical semantic fold.
async fn save_sixteen_publications(storage: &RecordStorage, id: &SessionId) {
    save_publications(storage, id, 16).await;
}
async fn save_publications(storage: &RecordStorage, id: &SessionId, count: u64) {
    let lease = storage.open(id.clone()).await.unwrap();
    let mut binding = lease.load().await.unwrap().binding().clone();
    let mut snapshot: Option<SessionSnapshot> = None;
    for generation in 0..count {
        let change = if generation == 0 {
            SessionChange::Opened {
                id: id.clone(),
                provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
                context: ProviderContext::Absent,
            }
        } else {
            SessionChange::ProviderContext {
                before: snapshot.as_ref().unwrap().provider_context.clone(),
                after: ProviderContext::Recorded(
                    ExecutionSessionId::new(format!("context-{generation}")).unwrap(),
                ),
            }
        };
        let next = records::fold_changes(snapshot.as_ref(), std::slice::from_ref(&change)).unwrap();
        let receipt = lease
            .save_changes(
                binding.clone(),
                next.clone(),
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        binding = receipt.next_for(&binding, 1).unwrap();
        snapshot = Some(next);
    }
    assert_eq!(binding.base(), 2 * count);
}

#[tokio::test]
async fn fresh_reader_pages_older_publication_after_another_reader_proves_newer_head() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("older-page").unwrap();
    save_sixteen_publications(&storage, &id).await;
    let mut newer = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let mut older = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    tokio::task::spawn_blocking(move || {
        let scope = newer.scope(sid("newer"), sid("epoch"));
        assert_eq!(newer.head(&scope), Ok(32));
        let mut request = PageRequest {
            scope: older.scope(sid("older"), sid("epoch")),
            after: 0,
            target: 20,
            max_records: 16,
            max_payload_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            max_record_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        };
        let page = older.page(&request);
        assert!(
            page.is_ok(),
            "unchanged stream serves publication20: {page:?}"
        );
        let page = page.unwrap();
        assert_eq!(page.request.target, 20);
        assert_eq!(page.records.last().unwrap().position, 16);
        request.target = 19;
        assert_eq!(older.page(&request), Err(SourceError::InvalidRequest));
        assert_eq!(newer.head(&scope), Ok(32));
    })
    .await
    .unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn proven_historical_pages_share_completion_evidence_without_prefix_replay() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("proven-pages").unwrap();
    save_sixteen_publications(&storage, &id).await;
    let mut newer = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let mut older = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let cache = storage.terminal_cache.clone();
    tokio::task::spawn_blocking(move || {
        let scope = newer.scope(sid("newer"), sid("epoch"));
        assert_eq!(newer.head(&scope), Ok(32));
        let before = cache.returned_records.load(Ordering::SeqCst);
        assert_eq!(before, 32);
        for after in [0, 8, 16] {
            let request = PageRequest {
                scope: older.scope(sid("older"), sid("epoch")),
                after,
                target: 20,
                max_records: 4,
                max_payload_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                max_record_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            };
            let page = older.bounded_page(&request).unwrap();
            assert!(
                matches!(page, RecordReadStatus::Ready(_)),
                "proven publication20 is ready: {page:?}"
            );
            assert_eq!(cache.returned_records.load(Ordering::SeqCst), before);
            assert_eq!(newer.head(&scope), Ok(32));
            assert_eq!(cache.returned_records.load(Ordering::SeqCst), before);
        }
    })
    .await
    .unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn competing_historical_query_advances_original_scan_and_preserves_forward_head() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("competing-history").unwrap();
    // Ninety-six saves exceed the existing 64 remembered completions.
    // Completion40 is evicted; Unit11 has no completion proof.
    save_publications(&storage, &id, 96).await;
    let mut original = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let mut competing = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let cache = storage.terminal_cache.clone();
    tokio::task::spawn_blocking(move || {
        let scope = original.scope(sid("original"), sid("epoch"));
        assert_eq!(original.head(&scope), Ok(192));
        let before = cache.returned_records.load(Ordering::SeqCst);
        let request = PageRequest {
            scope,
            after: 0,
            target: 40,
            max_records: 4,
            max_payload_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            max_record_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        };
        assert_eq!(
            original.bounded_page(&request),
            Ok(RecordReadStatus::Preparing)
        );
        assert_eq!(cache.returned_records.load(Ordering::SeqCst) - before, 16);
        let mut other = request.clone();
        other.scope = competing.scope(sid("competing"), sid("epoch"));
        other.target = 11;
        assert_eq!(
            competing.bounded_page(&other),
            Ok(RecordReadStatus::Preparing),
            "another target advances the original finite proof before acquiring its own scan"
        );
        assert_eq!(cache.returned_records.load(Ordering::SeqCst) - before, 32);
        assert!(matches!(
            original.bounded_page(&request),
            Ok(RecordReadStatus::Ready(_))
        ));
        assert_eq!(cache.returned_records.load(Ordering::SeqCst) - before, 40);
        assert_eq!(original.head(&request.scope), Ok(192));
        assert_eq!(cache.returned_records.load(Ordering::SeqCst) - before, 40);
        assert_eq!(
            competing.bounded_page(&other),
            Err(SourceError::InvalidRequest)
        );
        assert_eq!(cache.returned_records.load(Ordering::SeqCst) - before, 51);
        assert!(matches!(
            original.bounded_page(&request),
            Ok(RecordReadStatus::Ready(_))
        ));
        assert_eq!(cache.returned_records.load(Ordering::SeqCst) - before, 51);
    })
    .await
    .unwrap();
    storage.shutdown().await.unwrap();
}
async fn target_page(
    storage: &RecordStorage,
    id: &SessionId,
    target: u64,
) -> Result<RecordReadStatus<Page>, SourceError> {
    let mut source = storage
        .record_source(id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let request = PageRequest {
        scope: source.scope(sid("receiver"), sid("epoch")),
        after: 0,
        target,
        max_records: 16,
        max_payload_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        max_record_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    };
    tokio::task::spawn_blocking(move || {
        thread::spawn(move || source.bounded_page(&request))
            .join()
            .unwrap()
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn shared_discovery_serves_smaller_publication_then_resumes_captured_head() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("shared-smaller").unwrap();
    save_sixteen_publications(&storage, &id).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        16
    );
    let RecordReadStatus::Ready(page) = target_page(&storage, &id, 20).await.unwrap() else {
        panic!("the next bounded read reaches the valid publication20")
    };
    assert_eq!(page.request.target, 20);
    assert_eq!(page.records.len(), 16);
    assert_eq!(page.records.last().unwrap().position, 16);
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        20
    );
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(32));
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        32
    );
    storage.shutdown().await.unwrap();
}
#[tokio::test]
async fn shared_discovery_refuses_intermediate_unit_and_keeps_larger_progress() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("shared-unit").unwrap();
    save_sixteen_publications(&storage, &id).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
    assert!(matches!(
        target_page(&storage, &id, 19).await,
        Err(SourceError::InvalidRequest)
    ));
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        19
    );
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(32));
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        32
    );
    storage.shutdown().await.unwrap();
}
#[tokio::test]
async fn shared_discovery_finishes_smaller_capture_before_larger_requests() {
    for head_first in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("records")).unwrap();
        let id = SessionId::new("shared-larger").unwrap();
        save_sixteen_publications(&storage, &id).await;
        assert!(matches!(
            target_page(&storage, &id, 20).await.unwrap(),
            RecordReadStatus::Preparing
        ));
        assert_eq!(
            storage
                .terminal_cache
                .returned_records
                .load(Ordering::SeqCst),
            16
        );
        if head_first {
            assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(20));
        } else {
            assert!(matches!(
                target_page(&storage, &id, 32).await.unwrap(),
                RecordReadStatus::Preparing
            ));
        }
        assert_eq!(
            storage
                .terminal_cache
                .returned_records
                .load(Ordering::SeqCst),
            20
        );
        let RecordReadStatus::Ready(page) = target_page(&storage, &id, 32).await.unwrap() else {
            panic!("larger publication resumes after the original smaller capture")
        };
        assert_eq!(page.request.target, 32);
        assert_eq!(
            storage
                .terminal_cache
                .returned_records
                .load(Ordering::SeqCst),
            32
        );
        assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(32));
        assert_eq!(
            storage
                .terminal_cache
                .returned_records
                .load(Ordering::SeqCst),
            32
        );
        storage.shutdown().await.unwrap();
    }
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
    let target = append_save(&storage, &stream, 0, 2 * 1024 * 1024).await;
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
        thread::spawn(move || {
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
        target = append_save(&storage, &stream, index, 8).await;
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
        258
    );
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(target));
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        258
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
    let first = append_save(&storage, &stream, 0, 2 * 1024 * 1024).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
    let later = append_save(&storage, &stream, 1, 8).await;
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
    let [unit, complete] = save_facts(&stream, 0, 0, 2 * 1024 * 1024);
    let frames = stream_fact::frame_fact(&unit, 1).unwrap();
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
        thread::spawn(move || source.bounded_page(&request))
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
    loop {
        match head(&storage, &id).await {
            RecordReadStatus::Preparing => {}
            RecordReadStatus::Ready(value) => {
                assert_eq!(value, 0, "a Unit seal is not a save completion");
                break;
            }
        }
    }
    // The deliberately invalid historical target has its own bounded validation
    // reads. Measure only the new completion, against the now captured Unit tail.
    let before_completion = storage
        .terminal_cache
        .returned_records
        .load(Ordering::SeqCst);
    let terminal = stream_fact::commit_fact(
        runtime,
        &stream,
        &Cursor::new(stream.clone(), frames.len() as u64),
        &complete,
    )
    .await
    .unwrap();
    assert_eq!(
        head(&storage, &id).await,
        RecordReadStatus::Ready(terminal.offset)
    );
    assert_eq!(
        storage
            .terminal_cache
            .returned_records
            .load(Ordering::SeqCst),
        before_completion + 1
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
    let target = append_save(&storage, &stream, 0, 2 * 1024 * 1024).await;
    let source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    source.abandon_bounded_head_answer(scope);
    // Final drop on a non-entered thread joins admitted physical work even
    // though its reply consumer has gone away.
    tokio::task::spawn_blocking(move || thread::spawn(move || drop(source)).join().unwrap())
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
    append_save(&storage, &stream, 0, 8).await;
    let owner = storage.terminal_cache.acquire(&stream).unwrap();
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Preparing);
    let mut occupied = vec![owner];
    for index in 1..CACHE_ENTRIES {
        occupied.push(
            storage
                .terminal_cache
                .acquire(&StreamKey {
                    id: StreamId::new(format!("occupied-{index}")).unwrap(),
                    incarnation: IncarnationId([0; 16]),
                })
                .unwrap(),
        );
    }
    let another = SessionId::new("another").unwrap();
    let second = runtime
        .create_stream(&StreamId::new(another.as_str()).unwrap())
        .await
        .unwrap();
    append_save(&storage, &second, 0, 8).await;
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
    assert_eq!(head(&storage, &another).await, RecordReadStatus::Ready(2));
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
    append_save(&storage, &stream, 0, 8).await;
    let mut source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    let wrong = Scope::new(
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
        target: 2,
        max_records: 0,
        max_payload_bytes: 128,
        max_record_bytes: 128,
    };
    let failures = tokio::task::spawn_blocking(move || {
        thread::spawn(move || (source.bounded_head(&wrong), source.bounded_page(&request)))
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
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(2));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn cached_old_incarnation_and_pruned_prefix_are_typed_refusals() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let runtime = storage.runtime().await.unwrap();
    let stream = runtime
        .create_stream(&StreamId::new(id.as_str()).unwrap())
        .await
        .unwrap();
    append_save(&storage, &stream, 0, 2 * 1024 * 1024).await;
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
        thread::spawn(move || old.bounded_head(&old_scope))
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
    append_save(&storage, &replacement, 0, 8).await;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(2));
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
        thread::spawn(move || source.bounded_head(&scope))
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
    let [unit, complete] = save_facts(&stream, 0, 0, 2 * 1024 * 1024);
    let frames = stream_fact::frame_fact(&unit, 1).unwrap();
    for frame in frames.iter().take(20) {
        runtime.append(&stream, frame.clone()).await.unwrap();
    }
    loop {
        if let RecordReadStatus::Ready(value) = head(&storage, &id).await {
            assert_eq!(value, 0);
            break;
        }
    }
    let aborted =
        stream_fact::abort_partial_fact(runtime, &stream, &Cursor::new(stream.clone(), 0))
            .await
            .unwrap()
            .offset;
    assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(0));
    // An Abort retires a physical attempt, not its original save binding. Retry
    // the exact original unit before its original completion can publish.
    let retry = stream_fact::commit_fact(
        runtime,
        &stream,
        &Cursor::new(stream.clone(), aborted),
        &unit,
    )
    .await
    .unwrap();
    let valid = stream_fact::commit_fact(runtime, &stream, &retry, &complete)
        .await
        .unwrap()
        .offset;
    while head(&storage, &id).await != RecordReadStatus::Ready(valid) {}
    let mut invalid = stream_fact::frame_fact(&save_facts(&stream, valid, 1, 8)[0], valid + 1)
        .unwrap()
        .remove(0);
    invalid.payload = Payload::copy_from_slice(&[99]);
    runtime.append(&stream, invalid).await.unwrap();
    let mut source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    let refusal = tokio::task::spawn_blocking(move || {
        thread::spawn(move || source.bounded_head(&scope))
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
        assert_eq!(saved.forward.groups.published(), valid);
        assert_eq!(
            saved.forward.validator.offset(),
            valid,
            "invalid framing does not publish progress"
        );
    }
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_completion_remains_refused_after_physical_validator_advanced() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let provider = ProviderIdentity::new("provider", "model", "workspace").unwrap();
    let opening = SessionChange::Opened {
        id: id.clone(),
        provider: provider.clone(),
        context: ProviderContext::Absent,
    };
    let snapshot = SessionSnapshot {
        id: id.clone(),
        provider,
        provider_context: ProviderContext::Absent,
        invocations: Vec::new(),
        queue_history: Vec::new(),
    };
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            snapshot,
            vec![SessionSaveUnit::new(vec![opening]).unwrap()],
        )
        .await
        .unwrap();
    drop(lease);
    let donor_root = directory.path().join("donor");
    let donor = RecordStorage::new(&donor_root).unwrap();
    let foreign = SessionId::new("foreign").unwrap();
    let donor_lease = donor.open(foreign.clone()).await.unwrap();
    let provider = ProviderIdentity::new("provider", "model", "workspace").unwrap();
    let opening = SessionChange::Opened {
        id: foreign.clone(),
        provider: provider.clone(),
        context: ProviderContext::Absent,
    };
    let snapshot = SessionSnapshot {
        id: foreign,
        provider,
        provider_context: ProviderContext::Absent,
        invocations: Vec::new(),
        queue_history: Vec::new(),
    };
    donor_lease
        .save_changes(
            donor_lease.load().await.unwrap().binding().clone(),
            snapshot,
            vec![SessionSaveUnit::new(vec![opening]).unwrap()],
        )
        .await
        .unwrap();
    drop(donor_lease);
    donor.shutdown().await.unwrap();
    let foreign_completion: Vec<u8> = Connection::open(donor_root.join("records.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT payload FROM event_records WHERE offset=?1",
            [2u64.to_be_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    // Keep the actual complete physical envelope at its generated position2.
    // Its foreign semantic save identity disagrees with this stream's original Unit.
    assert_eq!(
        Connection::open(directory.path().join("records/records.sqlite3"))
            .unwrap()
            .execute(
                "UPDATE event_records SET payload=?1 WHERE offset=?2",
                rusqlite::params![foreign_completion, 2u64.to_be_bytes().as_slice()],
            )
            .unwrap(),
        1
    );
    for attempt in 0..2 {
        let mut source = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        let scope = source.scope(sid("receiver"), sid("epoch"));
        let result = tokio::task::spawn_blocking(move || {
            thread::spawn(move || source.bounded_head(&scope))
                .join()
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(result, Err(SourceError::Unavailable), "attempt={attempt}");
        assert_eq!(
            storage
                .terminal_cache
                .returned_records
                .load(Ordering::SeqCst),
            2,
            "failed cached validation cannot silently accept the already-scanned tail"
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
    let mut invalid = stream_fact::frame_fact(&save_facts(&stream, 0, 0, 8)[0], 1)
        .unwrap()
        .remove(0);
    invalid.payload = Payload::copy_from_slice(&[]);
    let payload = vec![99; 1024 * 1024 - invalid.accounted_bytes()];
    invalid.payload = Payload::copy_from_slice(&payload);
    assert_eq!(invalid.accounted_bytes(), 1024 * 1024);
    runtime.append(&stream, invalid).await.unwrap();
    let mut source = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(sid("receiver"), sid("epoch"));
    let refusal = tokio::task::spawn_blocking(move || {
        thread::spawn(move || source.bounded_head(&scope))
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
        assert_eq!(
            (
                saved.forward.validator.offset(),
                saved.forward.groups.published()
            ),
            (0, 0)
        );
    }
    storage.shutdown().await.unwrap();
}

#[test]
fn cold_restart_child() {
    let Ok(root) = env::var("NESSA_315_RESTART_ROOT") else {
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
    let target = append_save(&storage, &stream, 0, 2 * 1024 * 1024).await;
    while head(&storage, &id).await != RecordReadStatus::Ready(target) {}
    storage.shutdown().await.unwrap();
    drop(storage);
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env::current_exe().unwrap())
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
        incarnation: IncarnationId([0; 16]),
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
    let result = panic::catch_unwind(move || {
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
        let mut frames = save_frames(&stream, 0, 0, 2 * 1024 * 1024, 1);
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
            frames[16 + first_count].payload = Payload::copy_from_slice(&[99]);
            // Preserve the original large charge so this malformed frame remains
            // a lookahead candidate instead of fitting the first returned page.
            frames[16 + first_count].payload = Payload::copy_from_slice(&vec![
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
            thread::spawn(move || source.bounded_head(&scope))
                .join()
                .unwrap()
        })
        .await
        .unwrap();
        if malformed {
            assert_eq!(result, Err(SourceError::Unavailable));
            let cache = storage.terminal_cache.entries.lock().unwrap();
            let progress = cache.front().unwrap().state.as_ref().unwrap();
            assert_eq!(
                progress.forward.validator.offset(),
                (16 + first_count) as u64
            );
            assert_eq!(progress.forward.groups.published(), 0);
        } else {
            assert!(matches!(result, Ok(RecordReadStatus::Ready(_))));
        }
        storage.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn historical_miss_preserves_known_forward_failure_and_clean_head() {
    for malformed in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("records");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("forward-failure").unwrap();
        save_publications(&storage, &id, 96).await;
        let newer = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        let mut newer = tokio::task::spawn_blocking(move || {
            let mut newer = newer;
            let scope = newer.scope(sid("newer"), sid("epoch"));
            assert_eq!(newer.head(&scope), Ok(192));
            newer
        })
        .await
        .unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let (snapshot, binding) = lease.load().await.unwrap().into_published(&id).unwrap();
        let snapshot = snapshot.unwrap();
        let change = SessionChange::ProviderContext {
            before: snapshot.provider_context.clone(),
            after: ProviderContext::Recorded(ExecutionSessionId::new("later-context").unwrap()),
        };
        let next = records::fold_changes(Some(&snapshot), std::slice::from_ref(&change)).unwrap();
        let receipt = lease
            .save_changes(
                binding.clone(),
                next,
                vec![SessionSaveUnit::new(vec![change]).unwrap()],
            )
            .await
            .unwrap();
        assert_eq!(receipt.next_for(&binding, 1).unwrap().base(), 194);
        drop(lease);
        if malformed {
            // Corrupt only a newly appended physical row, never a validated prefix.
            // Keep its original stream identity, offset, event id and schema.
            let changed = Connection::open(root.join("records.sqlite3"))
                .unwrap()
                .execute(
                    "UPDATE event_records SET payload = ?1 WHERE offset = ?2",
                    rusqlite::params![&[99u8][..], &193u64.to_be_bytes()[..]],
                )
                .unwrap();
            assert_eq!(changed, 1);
        }
        let mut historical = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        let cache = storage.terminal_cache.clone();
        tokio::task::spawn_blocking(move || {
            let scope = newer.scope(sid("newer"), sid("epoch"));
            let expected = if malformed {
                Err(SourceError::Unavailable)
            } else {
                Ok(RecordReadStatus::Ready(194))
            };
            assert_eq!(newer.bounded_head(&scope), expected);
            let request = PageRequest {
                scope: historical.scope(sid("historical"), sid("epoch")),
                after: 0,
                target: 40,
                max_records: 4,
                max_payload_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                max_record_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            };
            let before = cache.returned_records.load(Ordering::SeqCst);
            assert_eq!(
                historical.bounded_page(&request),
                Ok(RecordReadStatus::Preparing)
            );
            assert_eq!(
                historical.bounded_page(&request),
                Ok(RecordReadStatus::Preparing)
            );
            assert!(matches!(
                historical.bounded_page(&request),
                Ok(RecordReadStatus::Ready(_))
            ));
            assert_eq!(cache.returned_records.load(Ordering::SeqCst) - before, 40);
            for _ in 0..2 {
                assert_eq!(
                    newer.bounded_head(&scope),
                    expected,
                    "historical40 preserves the original newer-head result"
                );
                assert_eq!(
                    cache.returned_records.load(Ordering::SeqCst) - before,
                    40,
                    "sticky forward failure performs no additional discovery read"
                );
            }
        })
        .await
        .unwrap();
        storage.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn historical_scan_survives_public_source_drop_and_recreation() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let id = SessionId::new("historical-drop").unwrap();
    save_publications(&storage, &id, 96).await;
    let mut first = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let cache = storage.terminal_cache.clone();
    let request = tokio::task::spawn_blocking(move || {
        let scope = first.scope(sid("receiver"), sid("epoch"));
        assert_eq!(first.head(&scope), Ok(192));
        let request = PageRequest {
            scope,
            after: 0,
            target: 40,
            max_records: 4,
            max_payload_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            max_record_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        };
        assert_eq!(
            first.bounded_page(&request),
            Ok(RecordReadStatus::Preparing)
        );
        assert_eq!(cache.returned_records.load(Ordering::SeqCst), 208);
        let mut known = request.clone();
        known.target = 10;
        assert!(matches!(
            first.bounded_page(&known),
            Ok(RecordReadStatus::Ready(_))
        ));
        assert_eq!(cache.returned_records.load(Ordering::SeqCst), 208);
        drop(first);
        request
    })
    .await
    .unwrap();
    let mut replacement = storage
        .record_source(&id, sid("origin"))
        .await
        .unwrap()
        .unwrap();
    let cache = storage.terminal_cache.clone();
    tokio::task::spawn_blocking(move || {
        assert_eq!(
            replacement.bounded_page(&request),
            Ok(RecordReadStatus::Preparing)
        );
        assert_eq!(cache.returned_records.load(Ordering::SeqCst), 224);
        let page = replacement.bounded_page(&request);
        assert!(
            matches!(page, Ok(RecordReadStatus::Ready(_))),
            "replacement resumes retained historical40: {page:?}"
        );
        assert_eq!(cache.returned_records.load(Ordering::SeqCst), 232);
        assert_eq!(replacement.head(&request.scope), Ok(192));
        assert_eq!(cache.returned_records.load(Ordering::SeqCst), 232);
    })
    .await
    .unwrap();
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_completion_proof_refuses_reset_prune_and_same_incarnation_shrink() {
    for change in ["reset", "prune", "shrink"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("records");
        let storage = RecordStorage::new(&root).unwrap();
        let id = SessionId::new("physical-proof").unwrap();
        save_sixteen_publications(&storage, &id).await;
        let mut newer = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        let mut historical = storage
            .record_source(&id, sid("origin"))
            .await
            .unwrap()
            .unwrap();
        tokio::task::spawn_blocking(move || {
            let scope = newer.scope(sid("newer"), sid("epoch"));
            assert_eq!(newer.head(&scope), Ok(32));
        })
        .await
        .unwrap();
        let runtime = storage.runtime().await.unwrap();
        let stream = runtime
            .find_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let expected = match change {
            "reset" => {
                runtime
                    .change_lifecycle(LifecycleRequest {
                        operation_id: LifecycleOperationId::new("proof-reset").unwrap(),
                        expected: stream,
                        action: LifecycleAction::Reset,
                    })
                    .await
                    .unwrap();
                SourceError::IdentityChanged
            }
            "prune" => {
                runtime
                    .enable_retry_policy(EnableRetryPolicy {
                        operation_id: RetentionOperationId::new("proof-policy").unwrap(),
                        stream: stream.clone(),
                    })
                    .await
                    .unwrap();
                runtime
                    .advance_retention_floor(AdvanceRetentionFloor {
                        operation_id: RetentionOperationId::new("proof-prune").unwrap(),
                        stream: stream.clone(),
                        expected_floor: Cursor::new(stream.clone(), 0),
                        new_floor: Cursor::new(stream, 1),
                    })
                    .await
                    .unwrap();
                SourceError::Pruned
            }
            "shrink" => {
                let mut database = Connection::open(root.join("records.sqlite3")).unwrap();
                let transaction = database.transaction().unwrap();
                assert_eq!(
                    transaction
                        .execute(
                            "DELETE FROM event_records WHERE offset > ?1",
                            [30u64.to_be_bytes().as_slice()]
                        )
                        .unwrap(),
                    2
                );
                assert_eq!(
                    transaction
                        .execute(
                            "UPDATE event_streams SET tail = ?1",
                            [30u64.to_be_bytes().as_slice()]
                        )
                        .unwrap(),
                    1
                );
                transaction.commit().unwrap();
                SourceError::IdentityChanged
            }
            _ => unreachable!(),
        };
        let cache = storage.terminal_cache.clone();
        tokio::task::spawn_blocking(move || {
            let request = PageRequest {
                scope: historical.scope(sid("historical"), sid("epoch")),
                after: 0,
                target: 20,
                max_records: 4,
                max_payload_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                max_record_bytes: super::super::MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            };
            let before = cache.returned_records.load(Ordering::SeqCst);
            assert_eq!(
                historical.bounded_page(&request),
                Err(expected),
                "change={change}"
            );
            assert_eq!(cache.returned_records.load(Ordering::SeqCst), before);
        })
        .await
        .unwrap();
        if change == "reset" {
            assert_eq!(head(&storage, &id).await, RecordReadStatus::Ready(0));
        }
        storage.shutdown().await.unwrap();
    }
}
