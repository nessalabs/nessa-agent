//! Incremental command recovery validates only the suffix of a retained head.
use super::*;
use crate::application::agent_execution::sessions::SessionStorage;
use event_stream::Bounds;

fn creation(request: &str) -> CreationReceipt {
    CreationReceipt::accepted(CreationBinding::new(
        ActionContext::new("person", "panel", request).unwrap(),
        SessionId::new("target").unwrap(),
        [7; 32],
    ))
}
fn stop(request: &str) -> MutationReceipt {
    MutationReceipt::accepted(
        MutationBinding::new(
            ActionContext::new("person", "panel", request).unwrap(),
            MutationOperation::Stop,
            SessionId::new("target").unwrap(),
            "turn",
            [8; 32],
        )
        .unwrap(),
    )
}
async fn open(
    storage: &RecordStorage,
    request: &str,
    access: CommandAccess,
) -> Arc<dyn CreationStorageLease> {
    storage
        .open_command("person".into(), request.into(), access)
        .await
        .unwrap()
        .unwrap()
}
fn control_inner(storage: &RecordStorage) -> Arc<ControlInner> {
    storage
        .controls
        .states
        .lock()
        .unwrap()
        .get("person")
        .unwrap()
        .upgrade()
        .unwrap()
}
async fn stream(inner: &ControlInner) -> StreamKey {
    inner.state.lock().await.as_ref().unwrap().stream.clone()
}
fn reset_probe(inner: &ControlInner) {
    let mut probe = inner.replay_probe.lock().unwrap();
    probe.starts.clear();
    probe.decoded = 0;
}
fn assert_probe(inner: &ControlInner, starts: &[u64], decoded: usize) {
    let probe = inner.replay_probe.lock().unwrap();
    assert_eq!(probe.starts, starts);
    assert_eq!(probe.decoded, decoded);
}

#[tokio::test]
async fn dirty_recovery_reads_only_the_unverified_suffix_and_cold_open_reads_the_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let owner = open(&storage, "original", CommandAccess::Creation).await;
    let accepted = creation("original");
    owner.save(accepted.clone()).await.unwrap();
    // A retained owner keeps the already validated principal state alive.
    for index in 0..130 {
        let request = format!("prefix-{index}");
        let lease = open(&storage, &request, CommandAccess::Creation).await;
        lease.save(creation(&request)).await.unwrap();
    }
    let attempted = accepted.advance(CreationStage::Attempted).unwrap();
    owner.save(attempted.clone()).await.unwrap();
    let ready = attempted.advance(CreationStage::Ready).unwrap();
    let inner = control_inner(&storage);
    assert_eq!(
        inner.state.lock().await.as_ref().unwrap().verified.offset,
        132
    );
    reset_probe(&inner);
    *inner.publication_fault.lock().unwrap() = Some(PublicationFault::LoseReply);
    assert!(matches!(
        owner.save(ready.clone()).await,
        Err(CreationStorageError::Storage(StorageError::Io(_)))
    ));
    assert!(inner.state.lock().await.as_ref().unwrap().dirty);
    assert_eq!(owner.load("original").await.unwrap(), Some(ready.clone()));
    assert_probe(&inner, &[132], 1);
    assert_eq!(
        inner.state.lock().await.as_ref().unwrap().verified.offset,
        133
    );
    assert!(!inner.state.lock().await.as_ref().unwrap().dirty);
    drop((inner, owner));
    storage.shutdown().await.unwrap();
    drop(storage);

    // Idle state is not a resident cache. A fresh adapter checks the full prefix.
    let storage = RecordStorage::new(&root).unwrap();
    let reader = open(&storage, "original", CommandAccess::ReadOnly).await;
    assert_eq!(reader.load("original").await.unwrap(), Some(ready));
    let restored = control_inner(&storage);
    assert_probe(&restored, &[0, 64, 128], 133);
    drop((restored, reader));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn deduplicated_command_acknowledgement_does_not_rewind_verified_progress() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let first = open(&storage, "original", CommandAccess::Creation).await;
    let second = open(&storage, "other", CommandAccess::Stop).await;
    let accepted = creation("original");
    let stopping = stop("other");
    first.save(accepted.clone()).await.unwrap();
    second.save_mutation(stopping.clone()).await.unwrap();
    let inner = control_inner(&storage);
    reset_probe(&inner);
    // The original Accepted event lives at position 1, behind the validated head.
    first.save(accepted.clone()).await.unwrap();
    assert_eq!(
        inner.state.lock().await.as_ref().unwrap().verified.offset,
        2
    );
    assert_probe(&inner, &[], 0);
    let attempting = stopping.advance(MutationStage::Attempted, None).unwrap();
    second.save_mutation(attempting.clone()).await.unwrap();
    assert_eq!(
        inner.state.lock().await.as_ref().unwrap().verified.offset,
        3
    );
    *inner.publication_fault.lock().unwrap() = Some(PublicationFault::LoseReply);
    let attempted = accepted.advance(CreationStage::Attempted).unwrap();
    assert!(matches!(
        first.save(attempted.clone()).await,
        Err(CreationStorageError::Storage(StorageError::Io(_)))
    ));
    assert_eq!(first.load("original").await.unwrap(), Some(attempted));
    assert_probe(&inner, &[3], 1);
    assert_eq!(
        second.load_mutation("other").await.unwrap(),
        Some(attempting)
    );
    assert_eq!(
        inner
            .runtime
            .bounds(&stream(&inner).await)
            .await
            .unwrap()
            .tail
            .offset,
        4
    );
    drop((inner, first, second));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_suffix_retains_verified_prefix_and_never_skips_the_bad_record() {
    for violation in [
        "principal",
        "event-id",
        "version",
        "schema",
        "binding",
        "missing-acceptance",
        "family",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordStorage::new(directory.path().join("records")).unwrap();
        let owner = open(&storage, "original", CommandAccess::Creation).await;
        let original = creation("original");
        owner.save(original.clone()).await.unwrap();
        let inner = control_inner(&storage);
        let stream = stream(&inner).await;
        let suffix = creation("suffix");
        let attempted = suffix.advance(CreationStage::Attempted).unwrap();
        for receipt in [&suffix, &attempted] {
            inner
                .runtime
                .append(&stream, encode(receipt).unwrap())
                .await
                .unwrap();
        }
        let ready = attempted.advance(CreationStage::Ready).unwrap();
        let mut bad = match violation {
            "principal" => encode(&CreationReceipt::accepted(CreationBinding::new(
                ActionContext::new("other-person", "panel", "foreign").unwrap(),
                SessionId::new("target").unwrap(),
                [7; 32],
            )))
            .unwrap(),
            "binding" => encode(
                &CreationReceipt::accepted(CreationBinding::new(
                    suffix.binding().actor().clone(),
                    SessionId::new("target").unwrap(),
                    [9; 32],
                ))
                .advance(CreationStage::Attempted)
                .unwrap()
                .advance(CreationStage::Ready)
                .unwrap(),
            )
            .unwrap(),
            "missing-acceptance" => encode(
                &creation("unbound")
                    .advance(CreationStage::Attempted)
                    .unwrap(),
            )
            .unwrap(),
            "family" => encode_mutation(&stop("suffix")).unwrap(),
            _ => encode(&ready).unwrap(),
        };
        match violation {
            "event-id" => bad.id = EventId::new("forged-suffix-event").unwrap(),
            "version" => bad.schema.version = 2,
            "schema" => bad.schema.id = SchemaId::new("unknown.schema").unwrap(),
            _ => {}
        }
        inner.runtime.append(&stream, bad).await.unwrap();
        inner.state.lock().await.as_mut().unwrap().dirty = true;
        reset_probe(&inner);
        for attempt in 0..2 {
            let failed = owner.load("original").await;
            if violation == "principal" {
                assert!(
                    matches!(
                        failed,
                        Err(CreationStorageError::Storage(
                            StorageError::IdentityMismatch
                        ))
                    ),
                    "{violation}: {failed:?}"
                );
            } else {
                assert!(
                    matches!(
                        failed,
                        Err(CreationStorageError::Storage(StorageError::Corrupt(_)))
                    ),
                    "{violation}: {failed:?}"
                );
            }
            let state = inner.state.lock().await;
            let state = state.as_ref().unwrap();
            assert!(state.dirty);
            assert_eq!(state.verified.offset, 3, "{violation} attempt {attempt}");
            assert!(
                matches!(state.receipts.get("original"), Some(Held::Creation(receipt)) if receipt == &original)
            );
            assert!(
                matches!(state.receipts.get("suffix"), Some(Held::Creation(receipt)) if receipt == &attempted)
            );
        }
        assert_probe(&inner, &[1, 3], 4);
        assert_eq!(inner.runtime.bounds(&stream).await.unwrap().tail.offset, 4);
        drop((inner, owner));
        storage.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn interrupted_suffix_read_resumes_after_its_verified_page() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    let owner = open(&storage, "original", CommandAccess::Creation).await;
    let original = creation("original");
    owner.save(original.clone()).await.unwrap();
    let inner = control_inner(&storage);
    let stream = stream(&inner).await;
    for index in 0..66 {
        let request = format!("suffix-{index}");
        let event = if index % 2 == 0 {
            encode(&creation(&request)).unwrap()
        } else {
            encode_mutation(&stop(&request)).unwrap()
        };
        inner.runtime.append(&stream, event).await.unwrap();
    }
    inner.state.lock().await.as_mut().unwrap().dirty = true;
    reset_probe(&inner);
    inner.replay_probe.lock().unwrap().fail_before_page = Some(65);
    assert!(matches!(
        owner.load("original").await,
        Err(CreationStorageError::Storage(StorageError::Io(_)))
    ));
    {
        let state = inner.state.lock().await;
        let state = state.as_ref().unwrap();
        assert!(state.dirty);
        assert_eq!(state.verified.offset, 65);
        assert_eq!(state.receipts.len(), 65);
    }
    assert_probe(&inner, &[1, 65], 64);
    assert_eq!(owner.load("original").await.unwrap(), Some(original));
    assert_probe(&inner, &[1, 65, 65], 66);
    {
        let state = inner.state.lock().await;
        let state = state.as_ref().unwrap();
        assert!(!state.dirty);
        assert_eq!(state.verified.offset, 67);
        assert_eq!(state.receipts.len(), 67);
        for index in 0..66 {
            let request = format!("suffix-{index}");
            if index % 2 == 0 {
                assert!(
                    matches!(state.receipts.get(&request), Some(Held::Creation(receipt)) if receipt == &creation(&request))
                );
            } else {
                assert!(
                    matches!(state.receipts.get(&request), Some(Held::Mutation(receipt)) if receipt == &stop(&request))
                );
            }
        }
    }
    drop((inner, owner));
    storage.shutdown().await.unwrap();
    drop(storage);
    let restored = RecordStorage::new(&root).unwrap();
    let reader = open(&restored, "suffix-65", CommandAccess::ReadOnly).await;
    assert_eq!(
        reader.load_mutation("suffix-65").await.unwrap(),
        Some(stop("suffix-65"))
    );
    drop(reader);
    restored.shutdown().await.unwrap();
}

#[tokio::test]
async fn suffix_recovery_preserves_the_shared_receipt_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let owner = open(&storage, "original", CommandAccess::Creation).await;
    let original = creation("original");
    owner.save(original.clone()).await.unwrap();
    let inner = control_inner(&storage);
    let stream = stream(&inner).await;
    for index in 1..MAX_RECEIPTS - 1 {
        let request = format!("retained-{index}");
        inner
            .runtime
            .append(&stream, encode(&creation(&request)).unwrap())
            .await
            .unwrap();
    }
    inner.state.lock().await.as_mut().unwrap().dirty = true;
    assert_eq!(
        owner.load("original").await.unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        inner.state.lock().await.as_ref().unwrap().receipts.len(),
        MAX_RECEIPTS - 1
    );
    reset_probe(&inner);
    inner
        .runtime
        .append(&stream, encode_mutation(&stop("last-stop")).unwrap())
        .await
        .unwrap();
    inner
        .runtime
        .append(&stream, encode(&creation("overflow")).unwrap())
        .await
        .unwrap();
    inner.state.lock().await.as_mut().unwrap().dirty = true;
    for _ in 0..2 {
        assert_eq!(
            owner.load("original").await,
            Err(CreationStorageError::Storage(StorageError::TooLarge))
        );
        let state = inner.state.lock().await;
        let state = state.as_ref().unwrap();
        assert!(state.dirty);
        assert_eq!(state.receipts.len(), MAX_RECEIPTS);
        assert_eq!(state.verified.offset, MAX_RECEIPTS as u64);
        assert!(!state.receipts.contains_key("overflow"));
        assert!(
            matches!(state.receipts.get("original"), Some(Held::Creation(receipt)) if receipt == &original)
        );
        assert!(
            matches!(state.receipts.get("last-stop"), Some(Held::Mutation(receipt)) if receipt == &stop("last-stop"))
        );
    }
    assert_probe(&inner, &[4095, 4096], 3);
    drop((inner, owner));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn dirty_recovery_rejects_incompatible_bounds() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let owner = open(&storage, "original", CommandAccess::Creation).await;
    owner.save(creation("original")).await.unwrap();
    let inner = control_inner(&storage);
    let stream = stream(&inner).await;
    let bounds = inner.runtime.bounds(&stream).await.unwrap();
    {
        let state = inner.state.lock().await;
        let state = state.as_ref().unwrap();
        state.validate_bounds(&bounds).unwrap();
        for violation in [
            "floor",
            "tail-backwards",
            "tail-limit",
            "floor-stream",
            "tail-stream",
            "floor-version",
            "tail-version",
        ] {
            let mut bad: Bounds = bounds.clone();
            match violation {
                "floor" => bad.floor.offset = 1,
                "tail-backwards" => bad.tail.offset = 0,
                "tail-limit" => bad.tail.offset = (MAX_RECEIPTS * 3 + 1) as u64,
                "floor-stream" => bad.floor.stream.id = StreamId::new("other").unwrap(),
                "tail-stream" => bad.tail.stream.id = StreamId::new("other").unwrap(),
                "floor-version" => bad.floor.version = 0,
                "tail-version" => bad.tail.version = 0,
                _ => unreachable!(),
            }
            assert!(
                matches!(state.validate_bounds(&bad), Err(StorageError::Corrupt(_))),
                "{violation}"
            );
        }
    }
    // Release the async guard before adapter shutdown.
    drop((inner, owner));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn command_append_publication_rejects_inconsistent_acknowledgements() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let owner = open(&storage, "original", CommandAccess::Creation).await;
    let original = creation("original");
    owner.save(original.clone()).await.unwrap();
    let inner = control_inner(&storage);
    let stream = stream(&inner).await;
    let attempted = original.advance(CreationStage::Attempted).unwrap();
    let expected = encode(&attempted).unwrap();
    // Keep the real physical append separate from its cache publication so the
    // production validation helper can see deliberately inconsistent answers.
    let actual = inner
        .runtime
        .append(&stream, expected.clone())
        .await
        .unwrap();
    {
        let mut guard = inner.state.lock().await;
        let state = guard.as_mut().unwrap();
        state.dirty = true;
        for violation in [
            "event",
            "stream",
            "version",
            "zero",
            "gap",
            "limit",
            "inserted-old",
        ] {
            let mut record = (*actual.record).clone();
            match violation {
                "event" => record.event = encode(&creation("different-request")).unwrap(),
                "stream" => record.cursor.stream.id = StreamId::new("other").unwrap(),
                "version" => record.cursor.version = 0,
                "zero" => record.cursor.offset = 0,
                "gap" => record.cursor.offset = 3,
                "limit" => record.cursor.offset = (MAX_RECEIPTS * 3 + 1) as u64,
                "inserted-old" => record.cursor.offset = 1,
                _ => unreachable!(),
            }
            let malformed = AppendReceipt {
                record: Arc::new(record),
                kind: AppendKind::Inserted,
            };
            assert!(
                matches!(
                    state.publish_append(&malformed, &expected, Held::Creation(attempted.clone())),
                    Err(StorageError::Corrupt(_))
                ),
                "{violation}"
            );
            assert!(state.dirty);
            assert_eq!(state.verified.offset, 1, "{violation}");
            assert!(
                matches!(state.receipts.get("original"), Some(Held::Creation(receipt)) if receipt == &original)
            );
        }
        state
            .publish_append(&actual, &expected, Held::Creation(attempted.clone()))
            .unwrap();
        assert_eq!(state.verified.offset, 2);
    }
    assert_eq!(owner.load("original").await.unwrap(), Some(attempted));
    assert!(!inner.state.lock().await.as_ref().unwrap().dirty);
    assert_eq!(inner.runtime.bounds(&stream).await.unwrap().tail.offset, 2);
    drop((inner, owner));
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn suffix_fold_rejects_records_outside_its_captured_contiguous_history() {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let owner = open(&storage, "original", CommandAccess::Creation).await;
    let original = creation("original");
    owner.save(original.clone()).await.unwrap();
    let inner = control_inner(&storage);
    let stream = stream(&inner).await;
    let attempted = original.advance(CreationStage::Attempted).unwrap();
    let appended = inner
        .runtime
        .append(&stream, encode(&attempted).unwrap())
        .await
        .unwrap();
    {
        let mut guard = inner.state.lock().await;
        let state = guard.as_mut().unwrap();
        state.dirty = true;
        for violation in [
            "captured-tail",
            "skipped-position",
            "version",
            "incarnation",
        ] {
            let mut record = (*appended.record).clone();
            let mut through = record.cursor.clone();
            match violation {
                "captured-tail" => through.offset = 1,
                "skipped-position" => {
                    record.cursor.offset = 3;
                    through.offset = 3;
                }
                "version" => record.cursor.version = 0,
                "incarnation" => record.cursor.stream.incarnation.0[0] ^= 1,
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    state.fold_record(&record, &through, "person"),
                    Err(StorageError::Corrupt(_))
                ),
                "{violation}"
            );
            assert!(state.dirty);
            assert_eq!(state.verified.offset, 1, "{violation}");
            assert_eq!(state.receipts.len(), 1);
            assert!(
                matches!(state.receipts.get("original"), Some(Held::Creation(receipt)) if receipt == &original)
            );
        }
        state
            .fold_record(&appended.record, &appended.record.cursor, "person")
            .unwrap();
        assert_eq!(state.verified.offset, 2);
    }
    assert_eq!(owner.load("original").await.unwrap(), Some(attempted));
    drop((inner, owner));
    storage.shutdown().await.unwrap();
}
