//! Public writer binding, preflight, watch, physical interruption and retry behavior.

use super::{
    fixtures::{
        input_record, opening_completion_offset, partial_input, refuse_offset, rows, sql,
        with_input,
    },
    opened,
};
use nessa_sdk::{
    application::agent_execution::{
        executions::ExecutionRequest,
        providers::ProviderIdentity,
        sessions::{
            ChangeWatchState, CommittedChangeWatch, SessionChange, SessionLoadState,
            SessionSaveGeneration, SessionSaveUnit, SessionSnapshot, SessionStorage, StorageError,
        },
    },
    domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext, SessionId},
    infrastructure::session_storage::RecordStorage,
};
use rusqlite::Connection;
use std::{
    future::Future,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    task::{Context, Poll, Waker},
    time::Duration,
};

fn watch_ready(watch: &mut CommittedChangeWatch) -> ChangeWatchState {
    let mut wait = Box::pin(watch.changed());
    match wait.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(state) => state,
        Poll::Pending => panic!("expected a committed source notice"),
    }
}
fn watch_pending(watch: &mut CommittedChangeWatch) {
    let mut wait = Box::pin(watch.changed());
    assert!(matches!(
        wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
}

#[tokio::test]
async fn record_watch_stays_clean_after_unit_seal_until_original_completion_retry() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("unpublished-watched").unwrap();
    let mut watch = storage.watch_committed(&id).unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let original = lease.load().await.unwrap().binding().clone();
    let (change, observed) = opened(&id);
    let units = vec![SessionSaveUnit::new(vec![change]).unwrap()];
    let completion = opening_completion_offset(&id).await;
    refuse_offset(
        &root,
        "refuse_save_completion",
        original.base() + completion,
    );
    assert!(matches!(
        lease
            .save_changes(original.clone(), observed.clone(), units.clone())
            .await,
        Err(StorageError::Io(_))
    ));
    assert_eq!(rows(&root), 1, "Unit is durable but no completion exists");
    let unfinished = lease.load().await.unwrap();
    assert_eq!(unfinished.state(), SessionLoadState::Unfinished);
    assert_eq!(unfinished.binding(), &original);
    assert!(unfinished.snapshot().is_none());
    watch_pending(&mut watch);
    let prior = storage.read_committed(id.clone()).await.unwrap().unwrap();
    assert_eq!(prior.position(), 0);
    assert!(prior.snapshot().is_none());
    sql(&root, "DROP TRIGGER refuse_save_completion;");
    let changed_opening = SessionChange::Opened {
        id: id.clone(),
        provider: ProviderIdentity::new("changed-provider", "model", "workspace").unwrap(),
        context: ProviderContext::Absent,
    };
    let changed_candidate = SessionSnapshot {
        provider: ProviderIdentity::new("changed-provider", "model", "workspace").unwrap(),
        ..observed.clone()
    };
    assert!(matches!(
        lease
            .save_changes(
                original.clone(),
                changed_candidate,
                vec![SessionSaveUnit::new(vec![changed_opening]).unwrap()],
            )
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(
        rows(&root),
        1,
        "changed confirmed unit cannot append completion"
    );
    let unfinished = lease.load().await.unwrap();
    assert_eq!(unfinished.state(), SessionLoadState::Unfinished);
    assert_eq!(unfinished.binding(), &original);
    assert!(unfinished.snapshot().is_none());
    watch_pending(&mut watch);
    let receipt = lease
        .save_changes(original.clone(), observed.clone(), units.clone())
        .await
        .unwrap();
    assert_eq!(rows(&root), 2);
    assert_eq!(watch_ready(&mut watch), ChangeWatchState::Dirty);
    assert_eq!(
        storage
            .read_committed(id)
            .await
            .unwrap()
            .unwrap()
            .snapshot(),
        Some(&observed)
    );
    assert_eq!(
        lease.save_changes(original, observed, units).await.unwrap(),
        receipt
    );
    assert_eq!(rows(&root), 2);
    watch_pending(&mut watch);
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn load_binds_fresh_reopened_and_reset_writers_to_actual_stream_progress() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("generation-boundary").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (opened, initial) = opened(&id);
    let original = lease.load().await.unwrap().binding().clone();
    assert_eq!((original.base(), original.generation()), (0, 0));
    let skipped = SessionSaveGeneration::new(original.backend().clone(), original.base(), 1);
    assert!(matches!(
        lease
            .save_changes(
                skipped,
                initial.clone(),
                vec![SessionSaveUnit::new(vec![opened.clone()]).unwrap()]
            )
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(rows(&root), 0, "invalid binding refuses before append");
    assert!(lease.load().await.unwrap().snapshot().is_none());
    let first_receipt = lease
        .save_changes(
            original.clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![opened.clone()]).unwrap()],
        )
        .await
        .unwrap();
    drop(lease);

    let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
    let restored_binding = lease.load().await.unwrap().binding().clone();
    assert_eq!(&restored_binding, first_receipt.next());
    assert!(restored_binding.base() > 0);
    assert_eq!(restored_binding.generation(), 1);
    let context = SessionChange::ProviderContext {
        before: ProviderContext::Absent,
        after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
    };
    let restored = SessionSnapshot {
        provider_context: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        ..initial.clone()
    };
    let skipped = SessionSaveGeneration::new(
        restored_binding.backend().clone(),
        restored_binding.base(),
        restored_binding.generation() + 1,
    );
    assert!(matches!(
        lease
            .save_changes(
                skipped,
                restored.clone(),
                vec![SessionSaveUnit::new(vec![context.clone()]).unwrap()]
            )
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert!(matches!(
        lease
            .save_changes(
                original,
                restored.clone(),
                vec![SessionSaveUnit::new(vec![context.clone()]).unwrap()]
            )
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&initial));
    let receipt = lease
        .save_changes(
            restored_binding,
            restored,
            vec![SessionSaveUnit::new(vec![context]).unwrap()],
        )
        .await
        .unwrap();

    lease.erase().await.unwrap();
    let reset = lease.load().await.unwrap();
    assert!(reset.snapshot().is_none());
    assert_eq!(
        (reset.binding().base(), reset.binding().generation()),
        (0, 0)
    );
    assert_ne!(reset.binding().backend(), receipt.next().backend());
    assert!(matches!(
        lease
            .save_changes(
                receipt.next().clone(),
                initial.clone(),
                vec![SessionSaveUnit::new(vec![opened.clone()]).unwrap()]
            )
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert!(lease.load().await.unwrap().snapshot().is_none());
    lease
        .save_changes(
            reset.binding().clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![opened]).unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&initial));
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn empty_erase_invalidates_unpolled_original_save_before_new_work() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("empty-erase-binding").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (opened, initial) = opened(&id);
    let original = lease.load().await.unwrap().binding().clone();
    let mut watch = storage.watch_committed(&id).unwrap();
    watch_pending(&mut watch);
    let queued = lease.save_changes(
        original.clone(),
        initial.clone(),
        vec![SessionSaveUnit::new(vec![opened.clone()]).unwrap()],
    );

    lease.erase().await.unwrap();
    let reset = lease.load().await.unwrap();
    assert!(reset.snapshot().is_none());
    assert_eq!(
        (reset.binding().base(), reset.binding().generation()),
        (0, 0)
    );
    let reset_notice = Box::pin(watch.changed())
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    let old_result = queued.await;
    let rows_after_old = rows(&root);
    let new_receipt = lease
        .save_changes(
            reset.binding().clone(),
            initial.clone(),
            vec![SessionSaveUnit::new(vec![opened]).unwrap()],
        )
        .await
        .unwrap();
    assert!(matches!(
        storage.open(id.clone()).await,
        Err(StorageError::Busy)
    ));
    drop(lease);
    let reopened = storage.open_existing(id).await.unwrap().unwrap();
    let loaded = reopened.load().await.unwrap();
    assert_eq!(loaded.snapshot(), Some(&initial));
    assert_eq!(loaded.binding(), new_receipt.next());
    drop(reopened);
    storage.shutdown().await.unwrap();

    assert_ne!(
        reset.binding().backend(),
        original.backend(),
        "empty erase must replace the original binding"
    );
    assert!(
        matches!(old_result, Err(StorageError::Corrupt(_))),
        "old queued save must refuse before append"
    );
    assert_eq!(rows_after_old, 0);
    assert!(
        matches!(reset_notice, Poll::Ready(ChangeWatchState::Dirty)),
        "empty Reset must publish through its original hook"
    );
}

#[tokio::test]
async fn explicit_units_publish_one_direct_save_beyond_one_body_bound() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let storage = RecordStorage::new(&root).unwrap();
    let (_, opening) = opened(&SessionId::new("conversation").unwrap());
    let invocations = (0..41)
        .map(|index| {
            input_record(
                &format!("input-{index}"),
                ExecutionRequest::MAX_MESSAGE_BYTES,
            )
        })
        .collect();
    let observed = SessionSnapshot {
        invocations,
        ..opening
    };
    let id = observed.id.clone();
    let lease = storage.open(id.clone()).await.unwrap();
    let original = lease.load().await.unwrap().binding().clone();
    let units: Vec<_> = std::iter::once(SessionChange::Opened {
        id: observed.id.clone(),
        provider: observed.provider.clone(),
        context: observed.provider_context.clone(),
    })
    .chain(
        observed
            .invocations
            .iter()
            .cloned()
            .map(|record| SessionChange::InputAccepted(Box::new(record))),
    )
    .map(|change| SessionSaveUnit::new(vec![change]).unwrap())
    .collect();
    let indivisible = SessionSaveUnit::new(
        units
            .iter()
            .flat_map(|unit| unit.changes().iter().cloned())
            .collect(),
    )
    .unwrap();
    let refused = lease
        .save_changes(original.clone(), observed.clone(), vec![indivisible])
        .await;
    assert!(
        matches!(refused, Err(StorageError::TooLarge)),
        "actual refusal: {refused:?}"
    );
    assert_eq!(rows(&root), 0);
    let prior = lease.load().await.unwrap();
    assert_eq!(prior.state(), SessionLoadState::Published);
    assert!(prior.snapshot().is_none());
    assert_eq!(prior.binding(), &original);
    let receipt = lease
        .save_changes(original.clone(), observed.clone(), units.clone())
        .await
        .unwrap();
    let loaded = lease.load().await.unwrap();
    assert_eq!(loaded.state(), SessionLoadState::Published);
    assert_eq!(loaded.snapshot(), Some(&observed));
    assert_eq!(
        loaded.binding(),
        &receipt.next_for(&original, units.len()).unwrap()
    );
    let committed_rows = rows(&root);
    assert!(committed_rows > units.len() as i64);
    drop(lease);
    storage.shutdown().await.unwrap();
    drop(storage);
    let reopened = RecordStorage::new(&root).unwrap();
    let lease = reopened.open_existing(id).await.unwrap().unwrap();
    let restored = lease.load().await.unwrap();
    assert_eq!(restored.state(), SessionLoadState::Published);
    assert_eq!(restored.snapshot(), Some(&observed));
    assert_eq!(
        restored.binding(),
        &receipt.next_for(&original, units.len()).unwrap()
    );
    assert_eq!(
        lease.save_changes(original, observed, units).await.unwrap(),
        receipt
    );
    assert_eq!(rows(&root), committed_rows);
    drop(lease);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn valid_unit_plan_with_wrong_candidate_never_appends() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("whole-plan-preflight").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let original = lease.load().await.unwrap().binding().clone();
    let (opened, initial) = opened(&id);
    let units = vec![SessionSaveUnit::new(vec![opened.clone()]).unwrap()];
    let contradictory = SessionSnapshot {
        provider: ProviderIdentity::new("other-provider", "model", "workspace").unwrap(),
        ..initial.clone()
    };
    assert!(matches!(
        lease
            .save_changes(original.clone(), contradictory, units.clone())
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(
        rows(&root),
        0,
        "valid unit with wrong candidate must refuse before append"
    );
    assert!(lease.load().await.unwrap().snapshot().is_none());
    let initial_receipt = lease
        .save_changes(original, initial.clone(), units)
        .await
        .unwrap();
    assert_eq!(initial_receipt.units(), 1);
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&initial));
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn late_invalid_unit_never_appends_valid_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("late-unit-preflight").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let original = lease.load().await.unwrap().binding().clone();
    let (opened, initial) = opened(&id);
    let initial_receipt = lease
        .save_changes(
            original,
            initial.clone(),
            vec![SessionSaveUnit::new(vec![opened.clone()]).unwrap()],
        )
        .await
        .unwrap();
    let binding = initial_receipt.next().clone();
    let context = SessionChange::ProviderContext {
        before: ProviderContext::Absent,
        after: ProviderContext::Recorded(ExecutionSessionId::new("valid-prefix-context").unwrap()),
    };
    let candidate = SessionSnapshot {
        provider_context: ProviderContext::Recorded(
            ExecutionSessionId::new("valid-prefix-context").unwrap(),
        ),
        ..initial.clone()
    };
    let prefix = SessionSaveUnit::new(vec![context]).unwrap();
    let late = SessionSaveUnit::new(vec![opened]).unwrap();
    let before = rows(&root);
    assert!(matches!(
        lease
            .save_changes(
                binding.clone(),
                candidate.clone(),
                vec![prefix.clone(), late]
            )
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(
        rows(&root),
        before,
        "late invalid unit cannot append valid prefix"
    );
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&initial));
    let receipt = lease
        .save_changes(binding, candidate.clone(), vec![prefix])
        .await
        .unwrap();
    assert_eq!(receipt.units(), 1);
    assert_eq!(lease.load().await.unwrap().snapshot(), Some(&candidate));
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn killed_child_retains_original_unpublished_unit_retry() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("crash-unit").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (opened, prior) = opened(&id);
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            prior.clone(),
            vec![SessionSaveUnit::new(vec![opened]).unwrap()],
        )
        .await
        .unwrap();
    let original = lease.load().await.unwrap().binding().clone();
    drop(lease);
    storage.shutdown().await.unwrap();
    drop(storage);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "infrastructure::session_storage::record::child_unpublished_unit_crash_probe",
            "--ignored",
            "--nocapture",
        ])
        .env("NESSA_RECORD_CHILD_ROOT", &root)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let output = child.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let ready = BufReader::new(output)
            .lines()
            .any(|line| line.is_ok_and(|line| line == "NESSA_UNSEALED_PREFIX_DURABLE"));
        let _ = send.send(ready);
    });
    let ready = receive.recv_timeout(Duration::from_secs(15));
    // Always reap this original child, including a failed readiness boundary.
    let killed = child.kill();
    let status = child.wait();
    reader.join().unwrap();
    assert_eq!(ready, Ok(true));
    killed.unwrap();
    assert!(!status.unwrap().success());
    let reopened = RecordStorage::new(&root).unwrap();
    let lease = reopened.open_existing(id.clone()).await.unwrap().unwrap();
    let loaded = lease.load().await.unwrap();
    assert_eq!(loaded.state(), SessionLoadState::Unfinished);
    assert_eq!(loaded.binding(), &original);
    assert_eq!(loaded.snapshot(), Some(&prior));
    assert!(matches!(
        loaded.into_published(&id),
        Err(StorageError::Unresolved)
    ));
    let changed = partial_input(4 * 1024 * 1024 - 1);
    let changed_snapshot = with_input(&prior, &changed);
    let retained = rows(&root);
    assert!(matches!(
        lease
            .save_changes(
                original.clone(),
                changed_snapshot,
                vec![SessionSaveUnit::new(vec![changed]).unwrap()]
            )
            .await,
        Err(StorageError::Corrupt(_))
    ));
    assert_eq!(rows(&root), retained);
    let change = partial_input(4 * 1024 * 1024);
    let complete = with_input(&prior, &change);
    let receipt = lease
        .save_changes(
            original.clone(),
            complete.clone(),
            vec![SessionSaveUnit::new(vec![change.clone()]).unwrap()],
        )
        .await
        .unwrap();
    let published = lease.load().await.unwrap();
    assert_eq!(published.state(), SessionLoadState::Published);
    assert_eq!(published.snapshot(), Some(&complete));
    assert_eq!(
        published.binding(),
        &receipt.next_for(&original, 1).unwrap()
    );
    let committed = rows(&root);
    assert_eq!(
        lease
            .save_changes(
                original,
                complete,
                vec![SessionSaveUnit::new(vec![change]).unwrap()]
            )
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(rows(&root), committed);
    drop(lease);
    reopened.shutdown().await.unwrap();
}

#[ignore = "child process crash probe"]
#[tokio::test]
async fn child_unpublished_unit_crash_probe() {
    let root =
        PathBuf::from(std::env::var_os("NESSA_RECORD_CHILD_ROOT").expect("parent supplies path"));
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("crash-unit").unwrap();
    let lease = storage.open_existing(id.clone()).await.unwrap().unwrap();
    let loaded = lease.load().await.unwrap();
    let original = loaded.binding().clone();
    let prior = loaded.snapshot().unwrap();
    let change = partial_input(4 * 1024 * 1024);
    let candidate = with_input(prior, &change);
    let before = rows(&root);
    refuse_offset(&root, "refuse_third_physical_row", original.base() + 3);
    let result = lease
        .save_changes(
            original.clone(),
            candidate,
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await;
    // The same public writer emitted Start and its first Piece; no Unit seal exists.
    assert!(matches!(result, Err(StorageError::Io(_))));
    assert_eq!(rows(&root), before + 2);
    let database = Connection::open(root.join("records.sqlite3")).unwrap();
    for (offset, expected_schema) in [
        (original.base() + 1, "nessa.fact-start"),
        (original.base() + 2, "nessa.fact-piece"),
    ] {
        let schema: String = database
            .query_row(
                "SELECT schema_id FROM event_records WHERE offset=?1",
                [offset.to_be_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema, expected_schema);
    }
    let unfinished = lease.load().await.unwrap();
    assert_eq!(unfinished.state(), SessionLoadState::Unfinished);
    assert_eq!(unfinished.binding(), &original);
    assert_eq!(unfinished.snapshot(), Some(prior));
    let absent: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM event_records WHERE offset=?1",
            [(original.base() + 3).to_be_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(absent, 0);
    sql(&root, "DROP TRIGGER refuse_third_physical_row;");
    drop(database);
    println!("\nNESSA_UNSEALED_PREFIX_DURABLE");
    std::io::stdout().flush().unwrap();
    // Keep this original process and its actual storage/lease alive until killed.
    std::thread::park();
    drop(lease);
    drop(storage);
    panic!("parent must kill the original child before this continuation");
}

#[tokio::test]
async fn predecessor_semantic_sqlite_record_refuses_without_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let storage = RecordStorage::new(&root).unwrap();
    let id = SessionId::new("predecessor").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (change, snapshot) = opened(&id);
    let receipt = lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            snapshot.clone(),
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    drop(lease);
    storage.shutdown().await.unwrap();
    drop(storage);
    let database = Connection::open(root.join("records.sqlite3")).unwrap();
    let original: (String, Vec<u8>) = database
        .query_row(
            "SELECT event_id,payload FROM event_records WHERE offset=?1",
            [1u64.to_be_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    // Fixed handcrafted retired-shape input, not a historical producer capture.
    let unsupported = include_bytes!("fixtures/retired-opening.bin").as_slice();
    assert_eq!(database.execute(
        "UPDATE event_records SET event_id=?1,payload=?2 WHERE offset=?3",
        rusqlite::params!["nessa-fact-d819b917251c1029a94eeb8cafc383ec1e913cab2f92e9b3215c04cbaa787787-start", unsupported, 1u64.to_be_bytes().as_slice()]
    ).unwrap(), 1);
    let count = rows(&root);
    // Preserve every persisted record and stream field across both refused opens.
    let evidence = || {
        let records = database.prepare(
            "SELECT quote(stream_key)||'|'||quote(offset)||'|'||quote(event_id)||'|'||quote(schema_id)||'|'||quote(schema_version)||'|'||quote(payload) FROM event_records ORDER BY stream_key,offset"
        ).unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();
        let streams = database.prepare(
            "SELECT quote(stream_key)||'|'||quote(public_id)||'|'||quote(incarnation)||'|'||quote(floor)||'|'||quote(tail)||'|'||quote(retired) FROM event_streams ORDER BY stream_key"
        ).unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();
        (records, streams)
    };
    let retained = evidence();
    for _ in 0..2 {
        let reopened = RecordStorage::new(&root).unwrap();
        assert!(matches!(
            reopened.open_existing(id.clone()).await,
            Err(StorageError::Corrupt(_))
        ));
        reopened.shutdown().await.unwrap();
        assert_eq!(rows(&root), count);
        assert_eq!(evidence(), retained);
    }
    assert_eq!(
        database
            .execute(
                "UPDATE event_records SET event_id=?1,payload=?2 WHERE offset=?3",
                rusqlite::params![original.0, original.1, 1u64.to_be_bytes().as_slice()]
            )
            .unwrap(),
        1
    );
    drop(database);
    let accepted = RecordStorage::new(&root).unwrap();
    let lease = accepted.open_existing(id).await.unwrap().unwrap();
    let loaded = lease.load().await.unwrap();
    assert_eq!(loaded.snapshot(), Some(&snapshot));
    assert_eq!(loaded.binding(), receipt.next());
    drop(lease);
    accepted.shutdown().await.unwrap();
}
