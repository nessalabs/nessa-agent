//! Queue journal facts must agree with current lifecycle state and prior queue order.
use super::{
    custom_storage::assert_custom_retention_admission, journal_bytes, journal_path, snapshot,
    snapshot_json,
};
use nessa_local_storage as private;
use nessa_sdk::application::agent_execution::{
    executions::SubmissionMode,
    sessions::{
        InvocationSchedulingEvent, QueueHistoryRecord, SessionSnapshot, SessionStorage,
        StorageError,
    },
};
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, InvocationKind, InvocationStage, QueueMutation, QueueOrderChange,
    QueueRemovalCause, SchedulingCause,
};
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, LocalFileStorage};
use serde_json::{json, Value};

fn pending(ids: &[&str]) -> SessionSnapshot {
    let mut value = snapshot("queue-evidence");
    value.invocations.clear();
    for name in ids {
        let mut record = snapshot("queue-evidence").invocations.remove(0);
        record.request.execution_id = ExecutionId::new(*name).unwrap();
        record.events.clear();
        record.submission = SubmissionMode::Queued;
        record.scheduling = vec![InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(record.actor.clone()),
        }];
        value.queue_history.push(QueueHistoryRecord {
            mutation: QueueMutation::Admitted {
                id: record.request.execution_id.clone(),
                kind: InvocationKind::Queued,
            },
            actor: Some(record.actor.clone()),
            scheduling_length: Some(1),
        });
        value.invocations.push(record);
    }
    value
}
fn reorder(before: &[&str], after: &[&str]) -> QueueHistoryRecord {
    QueueHistoryRecord {
        mutation: QueueMutation::Reordered(
            QueueOrderChange::new(
                before
                    .iter()
                    .map(|name| (ExecutionId::new(*name).unwrap(), InvocationKind::Queued))
                    .collect(),
                after
                    .iter()
                    .map(|name| ExecutionId::new(*name).unwrap())
                    .collect(),
            )
            .unwrap(),
        ),
        actor: Some(snapshot("queue-evidence").invocations[0].actor.clone()),
        scheduling_length: None,
    }
}
async fn rejected_at_boundaries(
    valid: SessionSnapshot,
    invalid: SessionSnapshot,
    corrupt_wire: impl FnOnce(&mut Value),
) {
    let memory = InMemoryStorage::new();
    let lease = memory.open(invalid.id.clone()).await.unwrap();
    assert!(matches!(
        lease.save(invalid.clone()).await,
        Err(StorageError::Corrupt(_))
    ));
    assert_custom_retention_admission(invalid.clone(), false).await;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(&directory).unwrap();
    let file = storage.open(valid.id.clone()).await.unwrap();
    file.save(valid).await.unwrap();
    let path = journal_path(&directory, "queue-evidence");
    let original = std::fs::read(&path).unwrap();
    let mut wire = snapshot_json(&original).unwrap();
    corrupt_wire(&mut wire);
    std::fs::write(&path, journal_bytes(&wire).unwrap()).unwrap();
    assert!(matches!(file.load().await, Err(StorageError::Corrupt(_))));
}
#[tokio::test]
async fn cancelled_pending_input_requires_its_removal_evidence() {
    let mut valid = pending(&["cancelled"]);
    let record = &mut valid.invocations[0];
    record.scheduling.push(InvocationSchedulingEvent {
        before: Some(InvocationStage::Queued),
        stage: InvocationStage::Cancelled,
        cause: SchedulingCause::Withdrawn,
        ..record.scheduling[0].clone()
    });
    valid.queue_history.push(QueueHistoryRecord {
        mutation: QueueMutation::Removed {
            id: record.request.execution_id.clone(),
            cause: QueueRemovalCause::Withdrawn,
        },
        actor: Some(record.actor.clone()),
        scheduling_length: Some(2),
    });
    let mut invalid = valid.clone();
    invalid.queue_history.pop();
    rejected_at_boundaries(valid, invalid, |wire| {
        wire["queue_history"].as_array_mut().unwrap().pop();
    })
    .await;
}

#[tokio::test]
async fn successive_reorders_must_start_from_the_previous_result() {
    let mut valid = pending(&["a", "b"]);
    valid.queue_history.push(reorder(&["a", "b"], &["b", "a"]));
    valid.queue_history.push(reorder(&["b", "a"], &["a", "b"]));
    let mut invalid = valid.clone();
    invalid.queue_history[3] = invalid.queue_history[2].clone();
    rejected_at_boundaries(valid, invalid, |wire| {
        wire["queue_history"][3] = wire["queue_history"][2].clone();
    })
    .await;
}
#[tokio::test]
async fn reorder_must_include_every_pending_member() {
    let mut valid = pending(&["a", "b", "c"]);
    valid
        .queue_history
        .push(reorder(&["a", "b", "c"], &["b", "a", "c"]));
    let mut invalid = valid.clone();
    invalid.queue_history[3] = reorder(&["a", "b"], &["b", "a"]);
    rejected_at_boundaries(valid, invalid, |wire| {
        let change = &mut wire["queue_history"][3]["mutation"]["Reordered"];
        change["before"].as_array_mut().unwrap().pop();
        change["after"].as_array_mut().unwrap().pop();
    })
    .await;
}
#[tokio::test]
async fn selected_input_must_be_the_actual_queue_front() {
    let mut valid = pending(&["a", "b"]);
    valid.queue_history.push(QueueHistoryRecord {
        mutation: QueueMutation::Selected {
            id: ExecutionId::new("a").unwrap(),
        },
        actor: None,
        scheduling_length: Some(1),
    });
    let mut invalid = valid.clone();
    invalid.queue_history[2].mutation = QueueMutation::Selected {
        id: ExecutionId::new("b").unwrap(),
    };
    rejected_at_boundaries(valid, invalid, |wire| {
        wire["queue_history"][2]["mutation"]["Selected"]["id"] = json!("b");
    })
    .await;
}
#[tokio::test]
async fn restoration_clears_old_membership_without_replaying_unresolved_input() {
    let mut valid = pending(&["old"]);
    valid.queue_history.push(QueueHistoryRecord {
        mutation: QueueMutation::Restored,
        actor: None,
        scheduling_length: None,
    });
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(&directory).unwrap();
    let lease = storage.open(valid.id.clone()).await.unwrap();
    lease.save(valid.clone()).await.unwrap();
    let restored = lease.load().await.unwrap().unwrap();
    assert_eq!(restored.queue_history, valid.queue_history);
    assert!(restored.invocations[0].result.is_none());
    drop(lease);
    assert_custom_retention_admission(restored, true).await;
    let mut invalid = valid.clone();
    invalid.queue_history.push(QueueHistoryRecord {
        mutation: QueueMutation::Selected {
            id: ExecutionId::new("old").unwrap(),
        },
        actor: None,
        scheduling_length: Some(1),
    });
    rejected_at_boundaries(valid, invalid, |wire| {
        wire["queue_history"]
            .as_array_mut()
            .unwrap()
            .push(json!({"mutation":{"Selected":{"id":"old"}},"actor":null,"scheduling_length":1}));
    })
    .await;
}

#[tokio::test]
async fn a_selected_identity_cannot_be_admitted_and_selected_again() {
    let mut valid = pending(&["once"]);
    valid.queue_history.push(QueueHistoryRecord {
        mutation: QueueMutation::Selected {
            id: ExecutionId::new("once").unwrap(),
        },
        actor: None,
        scheduling_length: Some(1),
    });
    let mut invalid = valid.clone();
    invalid.queue_history.extend(valid.queue_history.clone());
    rejected_at_boundaries(valid, invalid, |wire| {
        let duplicate = wire["queue_history"].as_array().unwrap().clone();
        wire["queue_history"]
            .as_array_mut()
            .unwrap()
            .extend(duplicate);
    })
    .await;
}
