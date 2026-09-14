//! A terminal scheduling claim requires a retained outcome at every storage boundary.
use super::super::custom_storage::assert_custom_retention_admission;
use super::*;
use super::{journal_bytes, snapshot_json};
use nessa_sdk::application::agent_execution::sessions::SessionSnapshot;
use serde_json::Value;

#[tokio::test]
async fn settled_history_requires_result_before_save_load_or_provider_restore() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        for dispatched in [false, true] {
            let mut value = settled_snapshot(mode, dispatched);
            for storage in &stores {
                let lease = storage.open(value.id.clone()).await.unwrap();
                lease.save(value.clone()).await.unwrap();
                let mut invalid = value.clone();
                invalid.invocations[0].result = None;
                assert!(matches!(
                    lease.save(invalid).await,
                    Err(StorageError::Corrupt(_))
                ));
                assert_same(&lease.load().await.unwrap().unwrap(), &value);
            }
            let path = journal_path(&root.path().join("private"), "settlement");
            let original_bytes = std::fs::read(&path).unwrap();
            let mut json: Value = snapshot_json(&original_bytes).unwrap();
            json["invocations"][0]["result"] = Value::Null;
            let bytes = journal_bytes(&json).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            let lease = stores[1].open(value.id.clone()).await.unwrap();
            assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            drop(lease);
            std::fs::write(&path, original_bytes).unwrap();
            value.invocations[0].result = None;
            assert_custom_retention_admission(value, false).await;
        }
    }
}

fn settled_snapshot(mode: SubmissionMode, dispatched: bool) -> SessionSnapshot {
    let mut value = snapshot("settlement");
    let record = &mut value.invocations[0];
    record.events.clear();
    record.submission = mode;
    let first = InvocationSchedulingEvent {
        kind: if mode == SubmissionMode::Queued {
            InvocationKind::Queued
        } else {
            InvocationKind::Steering
        },
        ..submitted()
    };
    record.scheduling = vec![first.clone()];
    if dispatched {
        record.scheduling.push(InvocationSchedulingEvent {
            before: Some(InvocationStage::Queued),
            stage: InvocationStage::Running,
            cause: SchedulingCause::Dispatched,
            actor: None,
            ..first.clone()
        });
    }
    record.scheduling.push(InvocationSchedulingEvent {
        before: Some(if dispatched {
            InvocationStage::Running
        } else {
            InvocationStage::Queued
        }),
        stage: InvocationStage::Settled,
        cause: if dispatched {
            SchedulingCause::ExecutionFailed
        } else {
            SchedulingCause::DispatchFailed
        },
        actor: None,
        ..first
    });
    record.result = Some(Err(AgentError::Closed));
    value
}

#[tokio::test]
async fn failed_dispatch_rejects_successful_results_at_every_storage_boundary() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        for dispatched in [false, true] {
            let mut valid = settled_snapshot(mode, dispatched);
            valid.invocations[0].scheduling.last_mut().unwrap().cause =
                SchedulingCause::DispatchFailed;
            assert_custom_retention_admission(valid.clone(), true).await;
            for outcome in [
                ExecutionOutcome::Completed,
                ExecutionOutcome::OutputLimit,
                ExecutionOutcome::RequestLimit,
                ExecutionOutcome::Refused,
                ExecutionOutcome::Cancelled,
            ] {
                let mut invalid = valid.clone();
                invalid.invocations[0].result = Some(Ok(outcome));
                for storage in &stores {
                    let lease = storage.open(valid.id.clone()).await.unwrap();
                    lease.save(valid.clone()).await.unwrap();
                    assert!(matches!(lease.save(invalid.clone()).await, Err(StorageError::Corrupt(_))),
                        "failed dispatch cannot retain a successful result: {mode:?}, dispatched={dispatched}, {outcome:?}");
                    assert_same(&lease.load().await.unwrap().unwrap(), &valid);
                }
                // Tamper only the result on a valid stored dispatch failure.
                let path = journal_path(&root.path().join("private"), "settlement");
                let original_bytes = std::fs::read(&path).unwrap();
                let mut json: Value = snapshot_json(&original_bytes).unwrap();
                json["invocations"][0]["result"] =
                    serde_json::json!({"Ok": format!("{outcome:?}")});
                let bytes = journal_bytes(&json).unwrap();
                std::fs::write(&path, &bytes).unwrap();
                let lease = stores[1].open(valid.id.clone()).await.unwrap();
                assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                drop(lease);
                std::fs::write(&path, original_bytes).unwrap();
                assert_custom_retention_admission(invalid.clone(), false).await;
                if dispatched {
                    // The same exact successful result is valid when execution settled.
                    invalid.invocations[0].scheduling.last_mut().unwrap().cause =
                        SchedulingCause::ExecutionSettled;
                    for storage in &stores {
                        let lease = storage.open(invalid.id.clone()).await.unwrap();
                        lease.save(invalid.clone()).await.unwrap();
                        assert_same(&lease.load().await.unwrap().unwrap(), &invalid);
                    }
                    assert_custom_retention_admission(invalid, true).await;
                }
            }
        }
    }
}

#[tokio::test]
async fn execution_settled_requires_an_exact_outcome_across_storage_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        let valid = settled_snapshot(mode, true);
        assert_custom_retention_admission(valid.clone(), true).await;
        let mut invalid = valid.clone();
        invalid.invocations[0].scheduling.last_mut().unwrap().cause =
            SchedulingCause::ExecutionSettled;
        assert_custom_retention_admission(invalid.clone(), false).await;
        for storage in &stores {
            let lease = storage.open(valid.id.clone()).await.unwrap();
            lease.save(valid.clone()).await.unwrap();
            assert_same(&lease.load().await.unwrap().unwrap(), &valid);
            assert!(matches!(
                lease.save(invalid.clone()).await,
                Err(StorageError::Corrupt(_))
            ));
        }
        let path = journal_path(&root.path().join("private"), "settlement");
        let original = std::fs::read(&path).unwrap();
        let mut json = snapshot_json(&original).unwrap();
        json["invocations"][0]["scheduling"][2]["cause"] = serde_json::json!("ExecutionSettled");
        std::fs::write(&path, journal_bytes(&json).unwrap()).unwrap();
        let lease = stores[1].open(valid.id.clone()).await.unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        drop(lease);
        std::fs::write(&path, original).unwrap();
    }
}
