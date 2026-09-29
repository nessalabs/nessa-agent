//! A validated JSONL session becomes one sealed, inert baseline candidate.
use super::*;
use event_stream::{
    infrastructure::{MemoryStore, MemoryStoreOptions, SqliteOptions, SqliteStore},
    EventReader, EventRuntime, EventSink, NewEvent, PageLimits, Payload, Runtime, RuntimeConfig,
    SchemaId, SchemaRef, StreamId,
};
use nessa_sdk::application::agent_execution::providers::{ExecutionReport, ProviderSessionState};
use nessa_sdk::infrastructure::session_storage::{
    commit_baseline_candidate, decode_baseline, encode_baseline, load_baseline,
    BaselineImportError, BaselineLoad, BaselineSection,
};

#[tokio::test]
async fn saved_session_baseline_round_trip_preserves_input_observation_and_receipt() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let original = snapshot("baseline-round-trip");
    let lease = storage.open(original.id.clone()).await.unwrap();
    lease.save(original.clone()).await.unwrap();
    let saved = SessionSnapshot::load_saved(&*lease, &original.id)
        .await
        .unwrap()
        .unwrap();
    let export = encode_baseline(&saved).unwrap();
    assert_eq!(export.pieces[0].section, BaselineSection::Header);
    assert_eq!(export.pieces[1].section, BaselineSection::Queue);
    assert_eq!(export.pieces[2].section, BaselineSection::Invocation(0));
    let recovered = decode_baseline(&export).unwrap();
    assert_same(&recovered, &original);
    assert_eq!(
        recovered.invocations[0].acknowledgement,
        original.invocations[0].acknowledgement
    );
    assert_eq!(encode_baseline(&recovered).unwrap(), export);
}

#[test]
fn baseline_keeps_global_queue_membership_and_each_invocation_identity() {
    let source = super::queue_history::pending(&["one", "two"]);
    let recovered = decode_baseline(&encode_baseline(&source).unwrap()).unwrap();
    assert_eq!(recovered.queue_history, source.queue_history);
    assert_eq!(recovered.invocations.len(), 2);
    for (actual, expected) in recovered.invocations.iter().zip(&source.invocations) {
        assert_eq!(actual.request, expected.request);
        assert_eq!(actual.actor, expected.actor);
        assert_eq!(actual.acknowledgement, expected.acknowledgement);
        assert_eq!(actual.scheduling, expected.scheduling);
    }
}

#[test]
fn missing_changed_or_reordered_pieces_cannot_expose_a_baseline() {
    let mut source = snapshot("bounded-baseline");
    source.invocations[0].request.user_message =
        UserMessage::text_only(PromptText::new("x".repeat(2 * 64 * 1024)).unwrap());
    let export = encode_baseline(&source).unwrap();
    assert!(export.pieces.len() > 4);
    assert!(export
        .pieces
        .iter()
        .all(|piece| !piece.bytes.is_empty() && piece.bytes.len() <= 64 * 1024));
    assert_same(&decode_baseline(&export).unwrap(), &source);

    let mut missing = export.clone();
    missing.pieces.remove(3);
    assert!(matches!(
        decode_baseline(&missing),
        Err(StorageError::Corrupt(_))
    ));

    let mut changed = export.clone();
    changed.pieces[3].bytes[0] ^= 1;
    assert!(matches!(
        decode_baseline(&changed),
        Err(StorageError::Corrupt(_))
    ));

    let mut reordered = export.clone();
    reordered.pieces.swap(2, 3);
    assert!(matches!(
        decode_baseline(&reordered),
        Err(StorageError::Corrupt(_))
    ));
}

#[tokio::test]
async fn candidate_import_resumes_identical_prefix_and_confirms_one_seal() {
    let runtime =
        Runtime::<MemoryStore>::open(MemoryStoreOptions::default(), RuntimeConfig::default())
            .await
            .unwrap();
    let stream = runtime
        .create_stream(&StreamId::new("baseline-import").unwrap())
        .await
        .unwrap();
    let source = snapshot("baseline-import");
    let export = encode_baseline(&source).unwrap();
    let committed = commit_baseline_candidate(&runtime, &stream, &export)
        .await
        .unwrap();
    let first_tail = runtime.bounds(&stream).await.unwrap().tail.offset;
    assert_eq!(first_tail, export.pieces.len() as u64 + 2);
    assert_eq!(committed.offset, first_tail);
    commit_baseline_candidate(&runtime, &stream, &export)
        .await
        .unwrap();
    assert_eq!(
        runtime.bounds(&stream).await.unwrap().tail.offset,
        first_tail
    );
    let changed = encode_baseline(&snapshot("different-source")).unwrap();
    assert!(matches!(
        commit_baseline_candidate(&runtime, &stream, &changed).await,
        Err(BaselineImportError::ConflictingStream)
    ));
    runtime
        .shutdown(std::time::Duration::from_secs(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn foreign_record_blocks_import_before_a_piece_is_written() {
    let runtime =
        Runtime::<MemoryStore>::open(MemoryStoreOptions::default(), RuntimeConfig::default())
            .await
            .unwrap();
    let stream = runtime
        .create_stream(&StreamId::new("foreign-baseline").unwrap())
        .await
        .unwrap();
    runtime
        .append(
            &stream,
            NewEvent {
                id: event_stream::EventId::new("foreign").unwrap(),
                schema: SchemaRef {
                    id: SchemaId::new("foreign").unwrap(),
                    version: 1,
                },
                payload: Payload::copy_from_slice(b"foreign"),
            },
        )
        .await
        .unwrap();
    let export = encode_baseline(&snapshot("foreign-baseline")).unwrap();
    assert!(matches!(
        commit_baseline_candidate(&runtime, &stream, &export).await,
        Err(BaselineImportError::ConflictingStream)
    ));
    assert_eq!(runtime.bounds(&stream).await.unwrap().tail.offset, 1);
    runtime
        .shutdown(std::time::Duration::from_secs(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn malformed_opening_record_is_refused_before_any_baseline_is_exposed() {
    let runtime =
        Runtime::<MemoryStore>::open(MemoryStoreOptions::default(), RuntimeConfig::default())
            .await
            .unwrap();
    let complete = runtime
        .create_stream(&StreamId::new("opening-source").unwrap())
        .await
        .unwrap();
    let malformed = runtime
        .create_stream(&StreamId::new("opening-malformed").unwrap())
        .await
        .unwrap();
    let export = encode_baseline(&snapshot("opening-source")).unwrap();
    commit_baseline_candidate(&runtime, &complete, &export)
        .await
        .unwrap();
    let bounds = runtime.bounds(&complete).await.unwrap();
    let first = runtime
        .read_after(
            &bounds.floor,
            PageLimits {
                max_records: 1,
                max_bytes: 1024 * 1024,
            },
            None,
        )
        .await
        .unwrap()
        .records
        .remove(0);
    let mut changed = first.event.clone();
    let mut bytes = changed.payload.as_bytes().to_vec();
    bytes[0] = 2;
    changed.payload = Payload::copy_from_slice(&bytes);
    runtime.append(&malformed, changed).await.unwrap();
    assert!(matches!(
        load_baseline(&runtime, &malformed).await,
        Err(BaselineImportError::InvalidStream)
    ));
    runtime
        .shutdown(std::time::Duration::from_secs(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn interrupted_import_keeps_legacy_authority_until_matching_suffix_seals() {
    let runtime =
        Runtime::<MemoryStore>::open(MemoryStoreOptions::default(), RuntimeConfig::default())
            .await
            .unwrap();
    let source_stream = runtime
        .create_stream(&StreamId::new("complete-candidate").unwrap())
        .await
        .unwrap();
    let partial_stream = runtime
        .create_stream(&StreamId::new("partial-candidate").unwrap())
        .await
        .unwrap();
    let export = encode_baseline(&snapshot("interrupted-import")).unwrap();
    commit_baseline_candidate(&runtime, &source_stream, &export)
        .await
        .unwrap();
    let bounds = runtime.bounds(&source_stream).await.unwrap();
    let prefix = runtime
        .read_after(
            &bounds.floor,
            PageLimits {
                max_records: 2,
                max_bytes: 1024 * 1024,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(prefix.records.len(), 2);
    for record in prefix.records {
        runtime
            .append(&partial_stream, record.event.clone())
            .await
            .unwrap();
    }
    assert!(matches!(
        load_baseline(&runtime, &partial_stream).await.unwrap(),
        BaselineLoad::Partial
    ));
    commit_baseline_candidate(&runtime, &partial_stream, &export)
        .await
        .unwrap();
    assert_eq!(
        runtime.bounds(&partial_stream).await.unwrap().tail.offset,
        export.pieces.len() as u64 + 2
    );
    assert!(matches!(
        load_baseline(&runtime, &partial_stream).await.unwrap(),
        BaselineLoad::Sealed { .. }
    ));
    runtime
        .shutdown(std::time::Duration::from_secs(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn sealed_baseline_reconstructs_the_saved_session_after_sqlite_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("records.sqlite3");
    let stream_id = StreamId::new("restarted-baseline").unwrap();
    let mut source = snapshot("restarted-baseline");
    let actor = source.invocations[0].actor.clone();
    let submitted = InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(actor),
    };
    source.invocations[0].submission = SubmissionMode::Queued;
    source.invocations[0].acknowledgement = SubmissionAcknowledgement::Acknowledged;
    let execution_id = source.invocations[0].request.execution_id.clone();
    source.invocations[0].events.push(ExecutionEvent::new(
        execution_id,
        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
    ));
    source.invocations[0].provider_report = Some(ExecutionReport::new(
        Some(Ok(ExecutionOutcome::Completed)),
        None,
        ProviderSessionState::Usable,
    ));
    source.invocations[0].local_outcome = Some(ExecutionOutcome::Completed);
    source.invocations[0].result = Some(Err(AgentError::AuditFailure));
    source.invocations[0].scheduling = vec![
        submitted.clone(),
        InvocationSchedulingEvent {
            before: Some(InvocationStage::Queued),
            stage: InvocationStage::Running,
            cause: SchedulingCause::Dispatched,
            actor: None,
            ..submitted.clone()
        },
        InvocationSchedulingEvent {
            before: Some(InvocationStage::Running),
            stage: InvocationStage::Settled,
            cause: SchedulingCause::ExecutionSettled,
            actor: None,
            ..submitted
        },
    ];
    super::scheduling::fixture_dispatches(&mut source);
    let mut pending = snapshot("restarted-baseline").invocations.remove(0);
    pending.request.execution_id = ExecutionId::new("accepted-pending").unwrap();
    pending.events.clear();
    pending.submission = SubmissionMode::Queued;
    pending.scheduling = vec![InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(pending.actor.clone()),
    }];
    source.queue_history.push(QueueHistoryRecord {
        mutation: QueueMutation::Admitted {
            id: pending.request.execution_id.clone(),
            kind: InvocationKind::Queued,
        },
        actor: Some(pending.actor.clone()),
        scheduling_length: Some(1),
    });
    source.invocations.push(pending);
    let mut withdrawn = snapshot("restarted-baseline").invocations.remove(0);
    withdrawn.request.execution_id = ExecutionId::new("withdrawn-input").unwrap();
    withdrawn.events.clear();
    withdrawn.submission = SubmissionMode::Queued;
    let admitted = InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(withdrawn.actor.clone()),
    };
    withdrawn.scheduling = vec![
        admitted.clone(),
        InvocationSchedulingEvent {
            before: Some(InvocationStage::Queued),
            stage: InvocationStage::Cancelled,
            cause: SchedulingCause::Withdrawn,
            ..admitted
        },
    ];
    withdrawn.local_outcome = Some(ExecutionOutcome::Cancelled);
    withdrawn.result = Some(Ok(ExecutionOutcome::Cancelled));
    source.queue_history.push(QueueHistoryRecord {
        mutation: QueueMutation::Admitted {
            id: withdrawn.request.execution_id.clone(),
            kind: InvocationKind::Queued,
        },
        actor: Some(withdrawn.actor.clone()),
        scheduling_length: Some(1),
    });
    source.queue_history.push(QueueHistoryRecord {
        mutation: QueueMutation::Removed {
            id: withdrawn.request.execution_id.clone(),
            cause: QueueRemovalCause::Withdrawn,
        },
        actor: Some(withdrawn.actor.clone()),
        scheduling_length: Some(2),
    });
    source.invocations.push(withdrawn);
    let legacy_directory = directory.path().join("legacy");
    private::create_directory(&legacy_directory).unwrap();
    let legacy = LocalFileStorage::new(&legacy_directory).unwrap();
    let legacy_lease = legacy.open(source.id.clone()).await.unwrap();
    legacy_lease.save(source.clone()).await.unwrap();
    let saved = SessionSnapshot::load_saved(&*legacy_lease, &source.id)
        .await
        .unwrap()
        .unwrap();
    let export = encode_baseline(&saved).unwrap();
    let runtime = Runtime::<SqliteStore>::open(SqliteOptions::new(&path), RuntimeConfig::default())
        .await
        .unwrap();
    let stream = runtime.create_stream(&stream_id).await.unwrap();
    assert!(matches!(
        load_baseline(&runtime, &stream).await.unwrap(),
        BaselineLoad::Absent
    ));
    commit_baseline_candidate(&runtime, &stream, &export)
        .await
        .unwrap();
    drop(legacy_lease);
    assert!(
        runtime
            .shutdown(std::time::Duration::from_secs(5))
            .await
            .unwrap()
            .closed
    );
    drop(runtime);

    let reopened =
        Runtime::<SqliteStore>::open(SqliteOptions::new(&path), RuntimeConfig::default())
            .await
            .unwrap();
    let same_stream = reopened.create_stream(&stream_id).await.unwrap();
    assert_eq!(same_stream, stream);
    let BaselineLoad::Sealed {
        snapshot: recovered,
        cursor,
    } = load_baseline(&reopened, &same_stream).await.unwrap()
    else {
        panic!("sealed baseline missing after restart");
    };
    assert_same(&recovered, &source);
    assert_eq!(cursor.offset, export.pieces.len() as u64 + 2);
    assert!(
        reopened
            .shutdown(std::time::Duration::from_secs(5))
            .await
            .unwrap()
            .closed
    );
}
