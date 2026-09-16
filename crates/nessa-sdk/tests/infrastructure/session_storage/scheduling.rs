use super::{
    custom_storage::assert_custom_retention_admission, journal_bytes, journal_path, snapshot_json,
};
mod local_cancellation;
mod outcome_history;
mod settlement;
mod undispatched;

use super::{assert_same, id, snapshot};
use nessa_local_storage as private;
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::{ExecutionEvent, SubmissionMode},
    permissions::ActionContext,
    sessions::{
        InvocationCancellationEvent, InvocationSchedulingEvent, QueueHistoryRecord,
        SessionSnapshot, SessionStorage, StorageError,
    },
};
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, InvocationKind, InvocationStage, QueueMutation, SchedulingCause,
};
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, LocalFileStorage};
use std::sync::Arc;

// These fixture invocations dispatch serially in vector order. This supplies
// explicit scheduler evidence; it is never inferred when loading real storage.
pub(super) fn fixture_dispatches(snapshot: &mut SessionSnapshot) {
    assert!(snapshot.queue_history.is_empty());
    for record in &snapshot.invocations {
        if let Some(index) = record
            .scheduling
            .iter()
            .position(|edge| edge.stage == InvocationStage::Running)
        {
            let id = record.request.execution_id.clone();
            snapshot.queue_history.push(QueueHistoryRecord {
                mutation: QueueMutation::Admitted {
                    id: id.clone(),
                    kind: record.scheduling[0].kind,
                },
                actor: Some(record.actor.clone()),
                scheduling_length: Some(1),
            });
            snapshot.queue_history.push(QueueHistoryRecord {
                mutation: QueueMutation::Selected { id },
                actor: None,
                scheduling_length: Some(index),
            });
        }
    }
}

fn submitted() -> InvocationSchedulingEvent {
    InvocationSchedulingEvent {
        kind: InvocationKind::Steering,
        target: None,
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(ActionContext::new("user", "test", "invoke").unwrap()),
    }
}

#[tokio::test]
async fn scheduling_round_trips_every_cause_stage_target_and_attribution() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let store = LocalFileStorage::new(root.path().join("private"))
        .unwrap()
        .open(id("scheduling"))
        .await
        .unwrap();
    let mut value = snapshot("scheduling");
    value.invocations.clear();
    for (stage, cause, actor) in [
        (InvocationStage::Running, SchedulingCause::Dispatched, None),
        (
            InvocationStage::Settled,
            SchedulingCause::DispatchFailed,
            None,
        ),
        (
            InvocationStage::Injected,
            SchedulingCause::SteeringInjected,
            None,
        ),
        (
            InvocationStage::Settled,
            SchedulingCause::ExecutionSettled,
            None,
        ),
        (
            InvocationStage::Settled,
            SchedulingCause::ExecutionFailed,
            None,
        ),
        (
            InvocationStage::Cancelled,
            SchedulingCause::SessionClosed,
            Some(ActionContext::new("owner", "surface", "close").unwrap()),
        ),
        (
            InvocationStage::Cancelled,
            SchedulingCause::RunnerStopped,
            None,
        ),
        (
            InvocationStage::Cancelled,
            SchedulingCause::Withdrawn,
            Some(ActionContext::new("owner", "surface", "withdraw").unwrap()),
        ),
    ] {
        let mut invocation = snapshot("scheduling").invocations.remove(0);
        invocation.submission = SubmissionMode::Steering;
        let first = InvocationSchedulingEvent {
            target: Some(ExecutionId::new("active").unwrap()),
            ..submitted()
        };
        invocation.scheduling = vec![
            first.clone(),
            InvocationSchedulingEvent {
                before: Some(first.stage),
                stage,
                cause,
                actor,
                ..first
            },
        ];
        if stage == InvocationStage::Settled {
            invocation.result = Some(Err(AgentError::Closed));
        }
        if cause == SchedulingCause::ExecutionSettled {
            invocation.result = Some(Ok(ExecutionOutcome::Completed));
        }
        if matches!(
            cause,
            SchedulingCause::ExecutionSettled | SchedulingCause::ExecutionFailed
        ) {
            invocation.scheduling[1].before = Some(InvocationStage::Running);
            invocation.scheduling.insert(
                1,
                InvocationSchedulingEvent {
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Running,
                    cause: SchedulingCause::Dispatched,
                    actor: None,
                    ..invocation.scheduling[0].clone()
                },
            );
        }
        if !invocation
            .scheduling
            .iter()
            .any(|event| event.stage == InvocationStage::Running)
        {
            // Pending, rejected, withdrawn, and injected submissions have no
            // provider output addressed to their own execution identity.
            invocation.events.clear();
        }
        value.invocations.push(invocation);
    }
    // Ordinary work has no active steering target.
    let mut queued = snapshot("scheduling").invocations.remove(0);
    queued.submission = SubmissionMode::Queued;
    queued.events.clear();
    queued.scheduling.push(InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        ..submitted()
    });
    value.invocations.push(queued);
    for (index, record) in value.invocations.iter_mut().enumerate() {
        record.request.execution_id = ExecutionId::new(format!("scheduling-case-{index}")).unwrap();
        record.events = record
            .events
            .iter()
            .map(|event| {
                ExecutionEvent::new(record.request.execution_id.clone(), event.update().clone())
            })
            .collect();
    }
    let mut active = snapshot("scheduling").invocations.remove(0);
    active.request.execution_id = ExecutionId::new("active").unwrap();
    // Native injection contributes observations to the already-running target.
    active.events = active
        .events
        .iter()
        .map(|event| {
            ExecutionEvent::new(active.request.execution_id.clone(), event.update().clone())
        })
        .collect();
    value.invocations.insert(0, active);
    fixture_dispatches(&mut value);
    store.save(value.clone()).await.unwrap();
    assert_same(&store.load().await.unwrap().unwrap(), &value);
}

#[tokio::test]
async fn scheduling_rejects_discontinuous_evidence_before_replacing_snapshot() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let store = storage.open(id("scheduling")).await.unwrap();
        let original = snapshot("scheduling");
        store.save(original.clone()).await.unwrap();
        for first in [true, false] {
            let mut invalid = original.clone();
            if !first {
                invalid.invocations[0].scheduling.push(submitted());
            }
            invalid.invocations[0]
                .scheduling
                .push(InvocationSchedulingEvent {
                    before: Some(InvocationStage::Running),
                    stage: InvocationStage::Settled,
                    cause: SchedulingCause::ExecutionSettled,
                    actor: None,
                    ..submitted()
                });
            assert!(matches!(
                store.save(invalid).await,
                Err(StorageError::Corrupt(_))
            ));
            assert_same(&store.load().await.unwrap().unwrap(), &original);
        }
    }
}

#[tokio::test]
async fn scheduling_json_rejects_invalid_target_actor_and_stage_chain() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let store = LocalFileStorage::new(root.path().join("private"))
        .unwrap()
        .open(id("scheduling"))
        .await
        .unwrap();
    let mut value = snapshot("scheduling");
    value.invocations[0].submission = SubmissionMode::Steering;
    value.invocations[0].scheduling.push(submitted());
    value.invocations[0].events.clear();
    store.save(value).await.unwrap();
    let path = journal_path(&root.path().join("private"), "scheduling");
    let original: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    for invalid in [0, 1, 2, 3] {
        let mut json = original.clone();
        let event = &mut json["invocations"][0]["scheduling"][0];
        match invalid {
            0 => event["target"] = "".into(),
            1 => event["actor"]["principal_id"] = "".into(),
            2 => event["before"] = "Running".into(),
            _ => event["cause"] = "Unknown".into(),
        }
        let bytes = journal_bytes(&json).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(store.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn original_submission_mode_survives_file_storage_for_retry_conflict_checks() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    for mode in [
        SubmissionMode::Immediate,
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        let lease = storage.open(id("intent")).await.unwrap();
        let mut value = snapshot("intent");
        value.invocations[0].submission = mode;
        if mode != SubmissionMode::Immediate {
            value.invocations[0].events.clear();
            let mut event = submitted();
            if mode == SubmissionMode::Queued {
                event.kind = InvocationKind::Queued;
                event.target = None;
            }
            value.invocations[0].scheduling.push(event);
        }
        lease.save(value.clone()).await.unwrap();
        drop(lease);
        let restored = storage.open(id("intent")).await.unwrap();
        assert_same(&restored.load().await.unwrap().unwrap(), &value);
    }
}

#[tokio::test]
async fn mismatched_submission_mode_is_rejected_before_save_and_on_read() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let lease = LocalFileStorage::new(root.path().join("private"))
        .unwrap()
        .open(id("intent"))
        .await
        .unwrap();
    let value = snapshot("intent");
    lease.save(value.clone()).await.unwrap();
    let mut invalid = value;
    invalid.invocations[0].submission = SubmissionMode::Queued;
    assert!(matches!(
        lease.save(invalid).await,
        Err(StorageError::Corrupt(_))
    ));
    let path = journal_path(&root.path().join("private"), "intent");
    let mut json: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    json["invocations"][0]["submission"] = "Queued".into();
    std::fs::write(path, journal_bytes(&json).unwrap()).unwrap();
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
}

#[tokio::test]
async fn storage_rejects_impossible_injection_and_cancellation_evidence() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let lease = storage.open(id("invalid-transitions")).await.unwrap();
        let original = snapshot("invalid-transitions");
        lease.save(original.clone()).await.unwrap();
        let first = submitted();
        for events in [
            vec![InvocationSchedulingEvent {
                stage: InvocationStage::Injected,
                cause: SchedulingCause::SteeringInjected,
                actor: None,
                ..first.clone()
            }],
            vec![
                first.clone(),
                InvocationSchedulingEvent {
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Injected,
                    cause: SchedulingCause::SteeringInjected,
                    actor: None,
                    target: None,
                    ..first.clone()
                },
            ],
            vec![
                first.clone(),
                InvocationSchedulingEvent {
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Cancelled,
                    cause: SchedulingCause::Withdrawn,
                    actor: None,
                    ..first.clone()
                },
            ],
        ] {
            let mut invalid = original.clone();
            invalid.invocations[0].submission = SubmissionMode::Steering;
            invalid.invocations[0].scheduling = events;
            assert!(matches!(
                lease.save(invalid).await,
                Err(StorageError::Corrupt(_))
            ));
            assert_same(&lease.load().await.unwrap().unwrap(), &original);
        }
    }
}

#[tokio::test]
async fn file_load_rejects_fabricated_injection_receipt() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let lease = LocalFileStorage::new(root.path().join("private"))
        .unwrap()
        .open(id("forged"))
        .await
        .unwrap();
    let mut original = snapshot("forged");
    original.invocations[0].submission = SubmissionMode::Steering;
    original.invocations[0].scheduling = vec![submitted()];
    original.invocations[0].events.clear();
    lease.save(original).await.unwrap();
    let path = journal_path(&root.path().join("private"), "forged");
    let mut json: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    json["invocations"][0]["scheduling"][0]["stage"] = "Injected".into();
    json["invocations"][0]["scheduling"][0]["cause"] = "SteeringInjected".into();
    json["invocations"][0]["scheduling"][0]["actor"] = serde_json::Value::Null;
    let bytes = journal_bytes(&json).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn scheduling_rejects_changed_unknown_self_targets_and_conflicting_admission_actor() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let lease = storage.open(id("correlation")).await.unwrap();
        let original = snapshot("correlation");
        lease.save(original.clone()).await.unwrap();
        for case in 0..6 {
            let mut value = original.clone();
            let mut record = original.invocations[0].clone();
            record.request.execution_id = ExecutionId::new("steering").unwrap();
            record.events.clear();
            record.submission = SubmissionMode::Steering;
            let target = ExecutionId::new(match case {
                1 => "unknown",
                2 => "steering",
                4 => "future",
                _ => "execution",
            })
            .unwrap();
            let first = InvocationSchedulingEvent {
                target: Some(target.clone()),
                ..submitted()
            };
            record.scheduling = vec![
                first.clone(),
                InvocationSchedulingEvent {
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Injected,
                    cause: SchedulingCause::SteeringInjected,
                    actor: None,
                    target: Some(if case == 0 {
                        ExecutionId::new("different").unwrap()
                    } else {
                        target
                    }),
                    ..first
                },
            ];
            if case == 3 {
                record.scheduling[0].actor =
                    Some(ActionContext::new("different", "test", "invoke").unwrap());
            }
            value.invocations.push(record);
            if case == 4 {
                let mut future = original.invocations[0].clone();
                future.request.execution_id = ExecutionId::new("future").unwrap();
                future.events.clear();
                value.invocations.push(future);
            }
            assert_custom_retention_admission(value.clone(), case == 5).await;
            if case == 5 {
                // A real prior invocation is accepted with the same native-delivery history.
                lease.save(value.clone()).await.unwrap();
                assert_same(&lease.load().await.unwrap().unwrap(), &value);
            } else {
                let result = lease.save(value).await;
                assert!(matches!(result, Err(StorageError::Corrupt(_))));
                assert_same(&lease.load().await.unwrap().unwrap(), &original);
            }
        }
    }
}

#[tokio::test]
async fn contradictory_scheduling_results_and_boundary_targets_preserve_prior_evidence() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let lease = storage.open(id("result-coherence")).await.unwrap();
        let original = snapshot("result-coherence");
        lease.save(original.clone()).await.unwrap();
        for case in 0..3 {
            let mut invalid = original.clone();
            let mut record = original.invocations[0].clone();
            record.request.execution_id = ExecutionId::new("submission").unwrap();
            record.events.clear();
            record.submission = match case {
                0 => SubmissionMode::Queued,
                1 => SubmissionMode::Steering,
                _ => SubmissionMode::BoundarySteering,
            };
            let first = InvocationSchedulingEvent {
                kind: if case == 0 {
                    InvocationKind::Queued
                } else {
                    InvocationKind::Steering
                },
                target: if case == 0 {
                    None
                } else {
                    Some(ExecutionId::new("execution").unwrap())
                },
                ..submitted()
            };
            record.scheduling = vec![first.clone()];
            if case != 2 {
                record.scheduling.push(InvocationSchedulingEvent {
                    before: Some(InvocationStage::Queued),
                    stage: if case == 0 {
                        InvocationStage::Cancelled
                    } else {
                        InvocationStage::Injected
                    },
                    cause: if case == 0 {
                        SchedulingCause::Withdrawn
                    } else {
                        SchedulingCause::SteeringInjected
                    },
                    actor: if case == 0 {
                        Some(record.actor.clone())
                    } else {
                        None
                    },
                    ..first
                });
                record.result = Some(Ok(ExecutionOutcome::Completed));
            }
            invalid.invocations.push(record);
            assert!(matches!(
                lease.save(invalid).await,
                Err(StorageError::Corrupt(_))
            ));
            assert_same(&lease.load().await.unwrap().unwrap(), &original);
        }
    }
}

#[tokio::test]
async fn decoded_scheduling_rejects_conflicting_results_and_boundary_targets() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let lease = LocalFileStorage::new(root.path().join("private"))
        .unwrap()
        .open(id("decoded-results"))
        .await
        .unwrap();
    let mut value = snapshot("decoded-results");
    let mut record = value.invocations[0].clone();
    record.request.execution_id = ExecutionId::new("submission").unwrap();
    record.events.clear();
    record.submission = SubmissionMode::Steering;
    let first = InvocationSchedulingEvent {
        target: Some(ExecutionId::new("execution").unwrap()),
        ..submitted()
    };
    record.scheduling = vec![
        first.clone(),
        InvocationSchedulingEvent {
            before: Some(InvocationStage::Queued),
            stage: InvocationStage::Injected,
            cause: SchedulingCause::SteeringInjected,
            actor: None,
            ..first
        },
    ];
    value.invocations.push(record);
    lease.save(value).await.unwrap();
    let path = journal_path(&root.path().join("private"), "decoded-results");
    let original: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    for case in 0..3 {
        let mut json = original.clone();
        let record = &mut json["invocations"][1];
        if case == 2 {
            record["submission"] = "BoundarySteering".into();
            record["scheduling"].as_array_mut().unwrap().truncate(1);
        } else {
            record["result"] = serde_json::json!({"Ok": "Completed"});
            if case == 0 {
                record["scheduling"][1]["stage"] = "Cancelled".into();
                record["scheduling"][1]["cause"] = "Withdrawn".into();
                record["scheduling"][1]["actor"] = record["actor"].clone();
            }
        }
        let bytes = journal_bytes(&json).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn scheduling_preserves_valid_transient_results_and_cancellation_outcomes() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        for (case, (stage, cause, result)) in [
            (
                InvocationStage::Queued,
                SchedulingCause::Submitted,
                Err(AgentError::Closed),
            ),
            (
                InvocationStage::Running,
                SchedulingCause::Dispatched,
                Ok(ExecutionOutcome::Completed),
            ),
            (
                InvocationStage::Cancelled,
                SchedulingCause::SessionClosed,
                Ok(ExecutionOutcome::Cancelled),
            ),
            (
                InvocationStage::Cancelled,
                SchedulingCause::SessionClosed,
                Err(AgentError::StorageAfterExecution {
                    error: StorageError::Io("failed settlement write".into()),
                    execution_result: Box::new(Ok(ExecutionOutcome::Cancelled)),
                }),
            ),
            (
                InvocationStage::Injected,
                SchedulingCause::SteeringInjected,
                Err(AgentError::Storage(StorageError::Io(
                    "failed injection write".into(),
                ))),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let key = format!("valid-results-{case}");
            let lease = storage.open(id(&key)).await.unwrap();
            let mut value = snapshot(&key);
            let mut record = value.invocations[0].clone();
            record.request.execution_id = ExecutionId::new("submission").unwrap();
            record.events.clear();
            record.submission = SubmissionMode::Steering;
            let first = InvocationSchedulingEvent {
                target: Some(ExecutionId::new("execution").unwrap()),
                ..submitted()
            };
            record.scheduling = vec![first.clone()];
            if stage != InvocationStage::Queued {
                record.scheduling.push(InvocationSchedulingEvent {
                    before: Some(InvocationStage::Queued),
                    stage,
                    cause,
                    actor: if stage == InvocationStage::Cancelled {
                        Some(record.actor.clone())
                    } else {
                        None
                    },
                    ..first
                });
            }
            record.result = Some(result);
            value.invocations.push(record);
            fixture_dispatches(&mut value);
            lease.save(value.clone()).await.unwrap();
            assert_same(&lease.load().await.unwrap().unwrap(), &value);
        }
    }
}

#[tokio::test]
async fn steering_targets_require_possible_dispatch_not_merely_prior_identity() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        for case in 0..8 {
            let key = format!("target-dispatch-{case}");
            let lease = storage.open(id(&key)).await.unwrap();
            let original = snapshot(&key);
            lease.save(original.clone()).await.unwrap();
            let accepted = matches!(case, 0 | 4 | 5 | 6);
            let mut value = original.clone();
            let target = &mut value.invocations[0];
            target.events.clear();
            if case == 1 {
                target.cancellation = Some(InvocationCancellationEvent {
                    cause: SchedulingCause::SessionClosed,
                    actor: Some(target.actor.clone()),
                });
            }
            if case >= 2 {
                target.submission = SubmissionMode::Queued;
                target.scheduling = vec![InvocationSchedulingEvent {
                    kind: InvocationKind::Queued,
                    ..submitted()
                }];
                if (4..=6).contains(&case) {
                    target.scheduling.push(InvocationSchedulingEvent {
                        kind: InvocationKind::Queued,
                        before: Some(InvocationStage::Queued),
                        stage: InvocationStage::Running,
                        cause: SchedulingCause::Dispatched,
                        actor: None,
                        ..submitted()
                    });
                }
                if matches!(case, 3 | 5 | 6) {
                    target.result = Some(Err(AgentError::Closed));
                    target.scheduling.push(InvocationSchedulingEvent {
                        kind: InvocationKind::Queued,
                        before: Some(if case == 3 {
                            InvocationStage::Queued
                        } else {
                            InvocationStage::Running
                        }),
                        stage: if case == 6 {
                            InvocationStage::Settled
                        } else {
                            InvocationStage::Cancelled
                        },
                        cause: if case == 6 {
                            SchedulingCause::DispatchFailed
                        } else {
                            SchedulingCause::SessionClosed
                        },
                        actor: if case == 6 {
                            None
                        } else {
                            Some(target.actor.clone())
                        },
                        ..submitted()
                    });
                }
            }
            if case == 7 {
                target.submission = SubmissionMode::Steering;
                target.scheduling = vec![
                    InvocationSchedulingEvent {
                        target: Some(ExecutionId::new("ancestor").unwrap()),
                        ..submitted()
                    },
                    InvocationSchedulingEvent {
                        target: Some(ExecutionId::new("ancestor").unwrap()),
                        before: Some(InvocationStage::Queued),
                        stage: InvocationStage::Injected,
                        cause: SchedulingCause::SteeringInjected,
                        actor: None,
                        ..submitted()
                    },
                ];
                let mut ancestor = original.invocations[0].clone();
                ancestor.request.execution_id = ExecutionId::new("ancestor").unwrap();
                ancestor.events.clear();
                value.invocations.insert(0, ancestor);
            }
            let mut child = original.invocations[0].clone();
            child.request.execution_id = ExecutionId::new("steering").unwrap();
            child.events.clear();
            child.submission = SubmissionMode::Steering;
            child.scheduling = vec![
                InvocationSchedulingEvent {
                    target: Some(ExecutionId::new("execution").unwrap()),
                    ..submitted()
                },
                InvocationSchedulingEvent {
                    target: Some(ExecutionId::new("execution").unwrap()),
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Injected,
                    cause: SchedulingCause::SteeringInjected,
                    actor: None,
                    ..submitted()
                },
            ];
            value.invocations.push(child);
            fixture_dispatches(&mut value);
            assert_custom_retention_admission(value.clone(), accepted).await;
            if accepted {
                lease.save(value.clone()).await.unwrap();
                assert_same(&lease.load().await.unwrap().unwrap(), &value);
            } else {
                assert!(matches!(
                    lease.save(value).await,
                    Err(StorageError::Corrupt(_))
                ));
                assert_same(&lease.load().await.unwrap().unwrap(), &original);
            }
        }
    }
}
