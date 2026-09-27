//! Exact local outcomes survive later failures and cannot claim undispatched work.
use super::super::custom_storage::assert_custom_retention_admission;
use super::super::{assert_same, snapshot};
use super::{fixture_dispatches, journal_bytes, journal_path, snapshot_json, submitted};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::{ExecutionEvent, ExecutionUpdate, SubmissionMode},
    providers::{ExecutionReport, ProviderSessionState},
    sessions::{InvocationSchedulingEvent, SessionSnapshot, SessionStorage, StorageError},
};
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionOutcome, InvocationKind, InvocationStage, SchedulingCause,
};
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, LocalFileStorage};
use serde_json::{json, Value};
use std::sync::Arc;

const OUTCOMES: [ExecutionOutcome; 5] = [
    ExecutionOutcome::Completed,
    ExecutionOutcome::Cancelled,
    ExecutionOutcome::Refused,
    ExecutionOutcome::OutputLimit,
    ExecutionOutcome::RequestLimit,
];

pub(super) fn history(mode: SubmissionMode, dispatched: bool) -> SessionSnapshot {
    let mut value = snapshot("outcome-history");
    let record = &mut value.invocations[0];
    record.events.clear();
    record.submission = mode;
    if mode != SubmissionMode::Immediate {
        let first = InvocationSchedulingEvent {
            kind: if mode == SubmissionMode::Queued {
                InvocationKind::Queued
            } else {
                InvocationKind::Steering
            },
            ..submitted()
        };
        record.scheduling.push(first.clone());
        if dispatched {
            record.scheduling.push(InvocationSchedulingEvent {
                before: Some(InvocationStage::Queued),
                stage: InvocationStage::Running,
                cause: SchedulingCause::Dispatched,
                actor: None,
                ..first
            });
        }
    }
    fixture_dispatches(&mut value);
    value
}

pub(super) async fn assert_accepted(value: SessionSnapshot) {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let lease = storage.open(value.id.clone()).await.unwrap();
        lease.save(value.clone()).await.unwrap();
        let loaded = lease.load().await.unwrap().unwrap();
        assert_same(&loaded, &value);
        assert_eq!(
            loaded.invocations[0].cancellation,
            value.invocations[0].cancellation
        );
        assert_eq!(
            loaded.invocations[0].local_outcome,
            value.invocations[0].local_outcome
        );
    }
    assert_custom_retention_admission(value, true).await;
}

pub(super) async fn assert_rejected(
    valid: SessionSnapshot,
    invalid: SessionSnapshot,
    corrupt: impl FnOnce(&mut Value),
) {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in &stores {
        let lease = storage.open(valid.id.clone()).await.unwrap();
        lease.save(valid.clone()).await.unwrap();
        assert!(matches!(
            lease.save(invalid.clone()).await,
            Err(StorageError::Corrupt(_))
        ));
        let loaded = lease.load().await.unwrap().unwrap();
        assert_same(&loaded, &valid);
        assert_eq!(
            loaded.invocations[0].local_outcome,
            valid.invocations[0].local_outcome
        );
    }
    assert_custom_retention_admission(invalid, false).await;
    let path = journal_path(&root.path().join("private"), "outcome-history");
    let mut wire = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    corrupt(&mut wire);
    let bytes = journal_bytes(&wire).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let lease = stores[1].open(valid.id).await.unwrap();
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "invalid complete evidence is never repaired"
    );
}

#[tokio::test]
async fn queued_success_requires_dispatch_while_immediate_and_explicit_withdrawal_remain_valid() {
    for outcome in OUTCOMES {
        let mut immediate = history(SubmissionMode::Immediate, false);
        immediate.invocations[0].local_outcome = Some(outcome);
        immediate.invocations[0].result = Some(Ok(outcome));
        assert_accepted(immediate).await;
        for mode in [
            SubmissionMode::Queued,
            SubmissionMode::BoundarySteering,
            SubmissionMode::Steering,
        ] {
            let valid = history(mode, false);
            let mut invalid = valid.clone();
            invalid.invocations[0].local_outcome = Some(outcome);
            invalid.invocations[0].result = Some(Ok(outcome));
            assert_rejected(valid, invalid, |wire| {
                wire["invocations"][0]["local_outcome"] = json!(format!("{outcome:?}"));
                wire["invocations"][0]["result"] = json!({"Ok": format!("{outcome:?}")});
            })
            .await;
            let mut withdrawn = history(mode, false);
            let first = withdrawn.invocations[0].scheduling[0].clone();
            withdrawn.invocations[0]
                .scheduling
                .push(InvocationSchedulingEvent {
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Cancelled,
                    cause: SchedulingCause::Withdrawn,
                    ..first
                });
            withdrawn.invocations[0].local_outcome = Some(ExecutionOutcome::Cancelled);
            withdrawn.invocations[0].result = Some(Ok(ExecutionOutcome::Cancelled));
            if outcome == ExecutionOutcome::Cancelled {
                assert_accepted(withdrawn).await;
            }
        }
    }
}

#[tokio::test]
async fn downgraded_local_success_rejects_later_conflicting_provider_and_terminal_outcomes() {
    for outcome in OUTCOMES {
        let different = if outcome == ExecutionOutcome::Completed {
            ExecutionOutcome::Refused
        } else {
            ExecutionOutcome::Completed
        };
        for source in ["provider", "terminal"] {
            let mut valid = history(SubmissionMode::Queued, true);
            valid.invocations[0].local_outcome = Some(outcome);
            valid.invocations[0].result = Some(Err(AgentError::AuditFailure));
            assert_accepted(valid.clone()).await;
            let mut invalid = valid.clone();
            if source == "provider" {
                invalid.invocations[0].provider_report = Some(ExecutionReport::new(
                    Some(Ok(different)),
                    None,
                    ProviderSessionState::Usable,
                ));
            } else {
                let execution_id = invalid.invocations[0].request.execution_id.clone();
                invalid.invocations[0].events.push(ExecutionEvent::new(
                    execution_id,
                    ExecutionUpdate::Finished(different),
                ));
            }
            assert_rejected(valid, invalid, |wire| {
                if source == "provider" {
                    wire["invocations"][0]["provider_report"] = json!({"Provider": {"result": {"Ok": format!("{different:?}")}, "failure": null, "attachment": "Usable"}});
                } else {
                    wire["invocations"][0]["events"] = json!([{"execution_id": "execution", "update": {"Finished": format!("{different:?}")}}]);
                }
            }).await;
        }
    }
}

#[tokio::test]
async fn failed_dispatch_cannot_retain_provider_completion_even_when_local_result_failed() {
    for outcome in OUTCOMES {
        for source in ["provider", "terminal"] {
            let mut valid = history(SubmissionMode::Queued, true);
            valid.invocations[0].result = Some(Err(AgentError::Closed));
            let first = valid.invocations[0].scheduling[0].clone();
            valid.invocations[0]
                .scheduling
                .push(InvocationSchedulingEvent {
                    before: Some(InvocationStage::Running),
                    stage: InvocationStage::Settled,
                    cause: SchedulingCause::DispatchFailed,
                    actor: None,
                    ..first
                });
            let mut invalid = valid.clone();
            if source == "provider" {
                invalid.invocations[0].provider_report = Some(ExecutionReport::new(
                    Some(Ok(outcome)),
                    None,
                    ProviderSessionState::Usable,
                ));
            } else {
                let execution_id = invalid.invocations[0].request.execution_id.clone();
                invalid.invocations[0].events.push(ExecutionEvent::new(
                    execution_id,
                    ExecutionUpdate::Finished(outcome),
                ));
            }
            assert_rejected(valid, invalid, |wire| {
                if source == "provider" {
                    wire["invocations"][0]["provider_report"] = json!({"Provider": {"result": {"Ok": format!("{outcome:?}")}, "failure": null, "attachment": "Usable"}});
                } else {
                    wire["invocations"][0]["events"] = json!([{"execution_id": "execution", "update": {"Finished": format!("{outcome:?}")}}]);
                }
            }).await;
        }
    }
}

#[tokio::test]
async fn saving_a_local_failure_after_success_preserves_the_first_outcome_across_reopen() {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let mut value = history(SubmissionMode::Queued, true);
        value.invocations[0].local_outcome = Some(ExecutionOutcome::Completed);
        value.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
        let lease = storage.open(value.id.clone()).await.unwrap();
        lease.save(value.clone()).await.unwrap();
        value.invocations[0].result = Some(Err(AgentError::AuditFailure));
        lease.save(value.clone()).await.unwrap();
        drop(lease);
        let lease = storage.open(value.id.clone()).await.unwrap();
        let loaded = lease.load().await.unwrap().unwrap();
        assert_eq!(
            loaded.invocations[0].local_outcome,
            Some(ExecutionOutcome::Completed)
        );
        assert_eq!(
            loaded.invocations[0].result,
            Some(Err(AgentError::AuditFailure))
        );
        assert_custom_retention_admission(loaded, true).await;
        value.invocations[0].provider_report = Some(ExecutionReport::new(
            Some(Ok(ExecutionOutcome::Refused)),
            None,
            ProviderSessionState::Usable,
        ));
        assert!(matches!(
            lease.save(value).await,
            Err(StorageError::Corrupt(_))
        ));
    }
}

#[tokio::test]
async fn retained_outcome_must_agree_with_current_success_and_requires_a_local_result() {
    for outcome in OUTCOMES {
        let mut valid = history(SubmissionMode::Immediate, false);
        valid.invocations[0].result = Some(Ok(outcome));
        assert_accepted(valid.clone()).await;
        let different = if outcome == ExecutionOutcome::Completed {
            ExecutionOutcome::Refused
        } else {
            ExecutionOutcome::Completed
        };
        let mut invalid = valid.clone();
        invalid.invocations[0].local_outcome = Some(different);
        assert_rejected(valid, invalid, |wire| {
            wire["invocations"][0]["local_outcome"] = json!(format!("{different:?}"));
        })
        .await;

        let valid = history(SubmissionMode::Immediate, false);
        let mut invalid = valid.clone();
        invalid.invocations[0].local_outcome = Some(outcome);
        assert_rejected(valid, invalid, |wire| {
            wire["invocations"][0]["local_outcome"] = json!(format!("{outcome:?}"));
        })
        .await;
    }
}
