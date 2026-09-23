//! Recovers an existing submission without repeating provider effects.
//! Saved intent verifies retries; live receipts share one settlement notification.

use super::scheduling::QueuedInvocation;
use super::{
    AdmissionEvidence, AdmissionEvidenceFailure, AgentError, QueueAdmission, SteeringDelivery,
    SteeringEvidence,
};
use crate::application::agent_execution::{
    executions::{ExecutionRequest, SubmissionMode},
    permissions::ActionContext,
    sessions::{SessionManager, SubmissionAcknowledgement},
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

fn admission_evidence(
    acknowledgement: &SubmissionAcknowledgement,
) -> Result<AdmissionEvidence, AgentError> {
    match acknowledgement {
        SubmissionAcknowledgement::Pending => Err(AgentError::SubmissionUnresolved),
        SubmissionAcknowledgement::Acknowledged => Ok(AdmissionEvidence::Acknowledged),
        SubmissionAcknowledgement::Failed { audit, storage } => {
            AdmissionEvidenceFailure::new(audit.clone(), storage.clone())
                .map(AdmissionEvidence::Failed)
                .ok_or(AgentError::SubmissionUnresolved)
        }
    }
}

fn steering_evidence(
    acknowledgement: &SubmissionAcknowledgement,
) -> Result<SteeringEvidence, AgentError> {
    admission_evidence(acknowledgement).map(|evidence| match evidence {
        AdmissionEvidence::Acknowledged => SteeringEvidence::Acknowledged,
        AdmissionEvidence::Failed(failure) => SteeringEvidence::Failed(failure),
    })
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
            // Provider acceptance is authoritative for delivery. A later local
            // stop or evidence failure cannot turn confirmed injection into a
            // retryable provider operation.
            return Some(event.target.clone().map_or_else(
                || Err(AgentError::SubmissionUnresolved),
                |target| {
                    steering_evidence(&record.acknowledgement)
                        .map(|evidence| SteeringDelivery::Injected { target, evidence })
                },
            ));
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
    Some(
        admission_evidence(&record.acknowledgement).and_then(|evidence| queued(receiver, evidence)),
    )
}
