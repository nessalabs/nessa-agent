//! Local cancellation and eventual provider completion remain separate durable facts.
use super::super::{assert_same, custom_storage::assert_custom_retention_admission};
use super::outcome_history::{assert_accepted, assert_rejected, history};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::{ExecutionEvent, ExecutionUpdate, SubmissionMode},
    permissions::ActionContext,
    providers::{CleanupReport, ExecutionReport, ProviderSessionState},
    sessions::{InvocationSchedulingEvent, SessionSnapshot, SessionStorage},
};
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionOutcome, InvocationStage, SchedulingCause,
};
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, LocalFileStorage};
use serde_json::json;
use std::sync::Arc;

fn cancel(value: &mut SessionSnapshot, cause: SchedulingCause, failed_local_save: bool) {
    let record = &mut value.invocations[0];
    record.scheduling.push(InvocationSchedulingEvent {
        before: Some(InvocationStage::Running),
        stage: InvocationStage::Cancelled,
        cause,
        actor: (cause == SchedulingCause::SessionClosed)
            .then(|| ActionContext::new("closer", "test", "close").unwrap()),
        ..record.scheduling[0].clone()
    });
    record.local_outcome = Some(ExecutionOutcome::Cancelled);
    record.result = Some(if failed_local_save {
        Err(AgentError::AuditFailure)
    } else {
        Ok(ExecutionOutcome::Cancelled)
    });
}

fn complete(value: &mut SessionSnapshot) {
    let record = &mut value.invocations[0];
    record.provider_report = Some(ExecutionReport::new(
        Some(Ok(ExecutionOutcome::Completed)),
        None,
        ProviderSessionState::Usable,
    ));
    record.events.push(ExecutionEvent::new(
        record.request.execution_id.clone(),
        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
    ));
}

#[tokio::test]
async fn cancellation_and_provider_completion_round_trip_both_save_orders_and_local_downgrades() {
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        for cause in [
            SchedulingCause::SessionClosed,
            SchedulingCause::RunnerStopped,
        ] {
            for provider_first in [false, true] {
                for failed_local_save in [false, true] {
                    let root = tempfile::tempdir().unwrap();
                    let stores: Vec<Arc<dyn SessionStorage>> = vec![
                        Arc::new(InMemoryStorage::new()),
                        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
                    ];
                    for storage in stores {
                        let mut value = history(mode, true);
                        let lease = storage.open(value.id.clone()).await.unwrap();
                        if provider_first {
                            complete(&mut value);
                        } else {
                            cancel(&mut value, cause, failed_local_save);
                        }
                        lease.save(value.clone()).await.unwrap();
                        assert_same(&lease.load().await.unwrap().unwrap(), &value);
                        if provider_first {
                            cancel(&mut value, cause, failed_local_save);
                        } else {
                            complete(&mut value);
                        }
                        lease.save(value.clone()).await.unwrap();
                        // Reopen so file restoration cannot rely only on the writer's cached snapshot.
                        drop(lease);
                        let lease = storage.open(value.id.clone()).await.unwrap();
                        let loaded = lease.load().await.unwrap().unwrap();
                        assert_same(&loaded, &value);
                        assert_eq!(
                            loaded.invocations[0].local_outcome,
                            Some(ExecutionOutcome::Cancelled)
                        );
                        assert_eq!(
                            loaded.invocations[0].provider_report,
                            value.invocations[0].provider_report
                        );
                        assert_eq!(
                            loaded.invocations[0].scheduling.last().unwrap().cause,
                            cause
                        );
                        assert_custom_retention_admission(loaded, true).await;
                        // A later local save diagnostic cannot erase either earlier fact.
                        value.invocations[0].result = Some(Err(AgentError::AuditFailure));
                        lease.save(value.clone()).await.unwrap();
                        let loaded = lease.load().await.unwrap().unwrap();
                        assert_same(&loaded, &value);
                        assert_eq!(
                            loaded.invocations[0].local_outcome,
                            Some(ExecutionOutcome::Cancelled)
                        );
                        assert_eq!(
                            loaded.invocations[0].provider_report,
                            value.invocations[0].provider_report
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn local_cancellation_cannot_hide_provider_audit_or_cleanup_failure() {
    for report in [
        ExecutionReport::new(
            Some(Err(AgentError::Protocol("provider failed".into()))),
            None,
            ProviderSessionState::Usable,
        ),
        ExecutionReport::new(
            Some(Ok(ExecutionOutcome::Completed)),
            Some(AgentError::AuditFailure),
            ProviderSessionState::Usable,
        ),
        ExecutionReport::new(
            Some(Ok(ExecutionOutcome::Completed)),
            None,
            ProviderSessionState::CleanupReported(CleanupReport::unconfirmed(
                AgentError::Protocol("cleanup failed".into()),
            )),
        ),
    ] {
        let mut valid = history(SubmissionMode::Queued, true);
        cancel(&mut valid, SchedulingCause::SessionClosed, true);
        valid.invocations[0].provider_report = Some(report);
        assert_accepted(valid.clone()).await;
        let mut invalid = valid.clone();
        invalid.invocations[0].result = Some(Ok(ExecutionOutcome::Cancelled));
        assert_rejected(valid, invalid, |wire| {
            wire["invocations"][0]["result"] = json!({"Ok": "Cancelled"});
        })
        .await;
    }
}
