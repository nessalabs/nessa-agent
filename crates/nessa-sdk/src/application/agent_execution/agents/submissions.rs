//! Recovers an existing submission without repeating provider effects.
//! Saved intent verifies retries; live receipts share one settlement notification.

use super::scheduling::QueuedInvocation;
use super::{AdmissionEvidence, AgentError, QueueAdmission, SteeringDelivery, SteeringEvidence};
use crate::application::agent_execution::{
    executions::{ExecutionRequest, SubmissionMode},
    permissions::ActionContext,
    sessions::SessionManager,
};
use crate::domain::agent_execution::executions::{ExecutionId, ExecutionOutcome, InvocationStage};
use std::collections::HashMap;
use tokio::sync::watch;

pub(super) type Settlement = Option<Result<ExecutionOutcome, AgentError>>;

pub(super) enum SubmissionReceipt {
    Queued {
        result: watch::Receiver<Settlement>,
        evidence: AdmissionEvidence,
    },
    Steering(Result<(ExecutionId, SteeringEvidence), AgentError>),
}

pub(super) async fn recover(
    manager: &SessionManager,
    receipts: &HashMap<ExecutionId, SubmissionReceipt>,
    input: &ExecutionRequest,
    actor: &ActionContext,
    mode: SubmissionMode,
) -> Option<Result<SteeringDelivery, AgentError>> {
    let record = manager.submission(&input.execution_id).await?;
    if record.request != *input || record.actor != *actor || record.submission != mode {
        return Some(Err(AgentError::SubmissionConflict));
    }
    let queued = |result, evidence| {
        Ok(SteeringDelivery::Queued(QueueAdmission::new(
            QueuedInvocation::new(input.execution_id.clone(), result),
            evidence,
        )))
    };
    if let Some(receipt) = receipts.get(&input.execution_id) {
        return Some(match receipt {
            SubmissionReceipt::Queued { result, evidence } => {
                queued(result.clone(), evidence.clone())
            }
            SubmissionReceipt::Steering(result) => result
                .clone()
                .map(|(target, evidence)| SteeringDelivery::Injected { target, evidence }),
        });
    }
    // Restored evidence is a receipt, never an instruction to replay provider work.
    let has_result = record.result.is_some();
    let result = match record.scheduling.last() {
        Some(event) if event.stage == InvocationStage::Injected => {
            if let Some(Err(error)) = record.result {
                return Some(Err(error));
            }
            return Some(
                event
                    .target
                    .clone()
                    .map(|target| SteeringDelivery::Injected {
                        target,
                        evidence: SteeringEvidence::Acknowledged,
                    })
                    .ok_or(AgentError::SubmissionUnresolved),
            );
        }
        Some(event) if event.stage == InvocationStage::Cancelled => {
            record.result.unwrap_or(Err(AgentError::Closed))
        }
        Some(event)
            if event.stage == InvocationStage::Settled
                || (event.stage == InvocationStage::Running && has_result) =>
        {
            record
                .result
                .unwrap_or(Err(AgentError::SubmissionUnresolved))
        }
        _ => return Some(Err(AgentError::SubmissionUnresolved)),
    };
    if mode == SubmissionMode::Steering
        && has_result
        && !record
            .scheduling
            .iter()
            .any(|event| event.stage == InvocationStage::Running)
    {
        return Some(Err(result
            .err()
            .unwrap_or(AgentError::SubmissionUnresolved)));
    }
    let (_, receiver) = watch::channel(Some(result));
    Some(queued(receiver, AdmissionEvidence::Acknowledged))
}
