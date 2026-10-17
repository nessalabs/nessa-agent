//! Recovers an existing submission without repeating provider effects.
//! Saved intent verifies retries; live receipts share one settlement notification.

use super::resumption::lineage::Lineage;
use super::scheduling::QueuedInvocation;
use super::{
    AdmissionEvidence, AdmissionEvidenceFailure, AgentError, QueueAdmission, SteeringDelivery,
    SteeringEvidence,
};
use crate::application::agent_execution::{
    executions::{ExecutionRequest, SubmissionMode, Submitter},
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

/// The receipt of the last authenticated successor of this original input.
async fn successor_receipt(
    manager: &SessionManager,
    receipts: &HashMap<ExecutionId, SubmissionReceipt>,
    input: &ExecutionRequest,
) -> Option<Result<SteeringDelivery, AgentError>> {
    let record = manager
        .with_invocations(|records| {
            let lineage = Lineage::new(records);
            let mut next = lineage.successor_of(&input.execution_id)?;
            while let Some(after) = lineage.successor_of(next) {
                next = after;
            }
            lineage.record(records, next).cloned()
        })
        .await?;
    let queued = |result, evidence| {
        Ok(SteeringDelivery::Queued(QueueAdmission::new(
            QueuedInvocation::new(input.execution_id.clone(), result),
            evidence,
        )))
    };
    if let Some(SubmissionReceipt::Queued { result, evidence }) =
        receipts.get(&record.request.execution_id)
    {
        return Some(queued(result.clone(), evidence.clone()));
    }
    let last = record.scheduling.last()?;
    let result = match last.stage {
        InvocationStage::Cancelled => record.result.unwrap_or(Err(AgentError::Closed)),
        _ => record
            .result
            .unwrap_or(Err(AgentError::SubmissionUnresolved)),
    };
    let (_, receiver) = watch::channel(Some(result));
    Some(
        admission_evidence(&record.acknowledgement).and_then(|evidence| queued(receiver, evidence)),
    )
}

pub(super) async fn recover(
    manager: &SessionManager,
    receipts: &HashMap<ExecutionId, SubmissionReceipt>,
    input: &ExecutionRequest,
    submitter: &Submitter,
    mode: SubmissionMode,
) -> Option<Result<SteeringDelivery, AgentError>> {
    let record = manager.submission(&input.execution_id).await?;
    // A retry under a different initiation is a different submission, the
    // same as one from a different actor.
    if record.request != *input
        || record.actor != submitter.context
        || record.initiation != submitter.initiation
        || record.submission != mode
    {
        return Some(Err(AgentError::SubmissionConflict));
    }
    let queued = |result, evidence| {
        Ok(SteeringDelivery::Queued(QueueAdmission::new(
            QueuedInvocation::new(input.execution_id.clone(), result),
            evidence,
        )))
    };
    // Owed work continues in a successor: a retry answers as that turn
    // does, even while the original's own receipt is still held
    // (`a_retry_of_a_resumed_turn_answers_as_its_successor`).
    if let Some(answer) = successor_receipt(manager, receipts, input).await {
        return Some(answer);
    }
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
