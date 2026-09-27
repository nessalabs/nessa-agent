//! Local cancellation is validated on every persistence and restoration boundary.
use super::outcome_history::{assert_accepted, assert_rejected, history};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::{ExecutionEvent, ExecutionUpdate, SubmissionMode},
    providers::{ExecutionReport, ProviderSessionState},
    sessions::InvocationCancellationEvent,
};
use nessa_sdk::domain::agent_execution::executions::{ExecutionOutcome, SchedulingCause};
use serde_json::json;

#[tokio::test]
async fn undispatched_cancellation_restores_its_cause_actor_and_rejects_provider_facts() {
    for (cause, explicit) in [
        (SchedulingCause::SessionClosed, true),
        (SchedulingCause::RunnerStopped, false),
    ] {
        let mut valid = history(SubmissionMode::Immediate, false);
        valid.invocations[0].cancellation = Some(InvocationCancellationEvent {
            cause,
            actor: explicit.then(|| valid.invocations[0].actor.clone()),
        });
        assert_accepted(valid.clone()).await;
        valid.invocations[0].result = Some(Err(AgentError::Closed));
        assert_accepted(valid.clone()).await;
        for source in ["output", "provider", "success", "actor"] {
            let mut invalid = valid.clone();
            let execution_id = invalid.invocations[0].request.execution_id.clone();
            match source {
                "output" => invalid.invocations[0].events.push(ExecutionEvent::new(
                    execution_id,
                    ExecutionUpdate::Finished(ExecutionOutcome::Cancelled),
                )),
                "provider" => {
                    invalid.invocations[0].provider_report = Some(ExecutionReport::new(
                        None,
                        None,
                        ProviderSessionState::Usable,
                    ))
                }
                "success" => invalid.invocations[0].result = Some(Ok(ExecutionOutcome::Completed)),
                _ => {
                    invalid.invocations[0].cancellation.as_mut().unwrap().actor = if explicit {
                        None
                    } else {
                        Some(valid.invocations[0].actor.clone())
                    }
                }
            }
            let actor =
                json!({"principal_id": "user", "surface_id": "test", "request_id": "invoke"});
            assert_rejected(valid.clone(), invalid, |wire| match source {
                "output" => {
                    wire["invocations"][0]["events"] =
                        json!([{"execution_id":"execution", "update":{"Finished":"Cancelled"}}])
                }
                "provider" => {
                    wire["invocations"][0]["provider_report"] =
                        json!({"Provider":{"result":null,"failure":null,"attachment":"Usable"}})
                }
                "success" => wire["invocations"][0]["result"] = json!({"Ok":"Completed"}),
                _ => {
                    wire["invocations"][0]["cancellation"]["actor"] =
                        if explicit { json!(null) } else { actor }
                }
            })
            .await;
        }
    }
}
