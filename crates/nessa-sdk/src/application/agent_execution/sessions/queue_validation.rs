//! Replay actual scheduler membership independently of later provider-dispatch edges.
use super::{QueueHistoryRecord, SessionSnapshot, StorageError};
use crate::application::agent_execution::executions::limits::validate_observation_id;
use crate::domain::agent_execution::executions::{
    InvocationQueue, InvocationStage, QueueMutation, QueueOrderChange, QueueRemovalCause,
    SchedulingCause,
};
use std::collections::{HashMap, HashSet};
fn corrupt(message: &str) -> StorageError {
    StorageError::Corrupt(message.into())
}
pub(super) fn replay(snapshot: &SessionSnapshot) -> Result<InvocationQueue, StorageError> {
    let maximum = snapshot
        .invocations
        .len()
        .saturating_mul(3)
        .saturating_add(QueueHistoryRecord::MAX_REORDERS);
    if snapshot.queue_history.len() > maximum
        || snapshot.queue_history.capacity() > maximum.saturating_mul(2)
    {
        return Err(corrupt("queue history exceeds its structural bound"));
    }
    let records: HashMap<_, _> = snapshot
        .invocations
        .iter()
        .map(|record| (&record.request.execution_id, record))
        .collect();
    let mut queue =
        InvocationQueue::new(QueueOrderChange::MAX_PENDING).expect("positive queue capacity");
    let mut selected = HashSet::new();
    let mut reorders = 0;
    for entry in &snapshot.queue_history {
        let checkpoint = if let Some(id) = entry.mutation.id() {
            validate_observation_id(id.as_str())
                .map_err(|_| corrupt("queue identity exceeds its bound"))?;
            let record = records
                .get(id)
                .ok_or_else(|| corrupt("queue event has no submitted invocation"))?;
            let index = entry
                .scheduling_length
                .and_then(|length| length.checked_sub(1))
                .ok_or_else(|| corrupt("queue event has no scheduling checkpoint"))?;
            Some((
                *record,
                record
                    .scheduling
                    .get(index)
                    .ok_or_else(|| corrupt("queue scheduling checkpoint is absent"))?,
            ))
        } else {
            if entry.scheduling_length.is_some() {
                return Err(corrupt("whole-queue mutation has an invocation checkpoint"));
            }
            None
        };
        match &entry.mutation {
            QueueMutation::Admitted { kind, .. } => {
                let (record, edge) = checkpoint.expect("single-input event");
                if edge.stage != InvocationStage::Queued
                    || edge.kind != *kind
                    || entry.actor.as_ref() != Some(&record.actor)
                {
                    return Err(corrupt("queue admission disagrees with submitted input"));
                }
            }
            QueueMutation::Selected { id } => {
                let (_, edge) = checkpoint.expect("single-input event");
                if edge.stage != InvocationStage::Queued || entry.actor.is_some() {
                    return Err(corrupt("queue selection disagrees with pending checkpoint"));
                }
                selected.insert(id);
            }
            QueueMutation::Removed { cause, .. } => {
                let (_, edge) = checkpoint.expect("single-input event");
                let (expected_stage, expected_cause) = match cause {
                    QueueRemovalCause::Withdrawn => {
                        (InvocationStage::Cancelled, SchedulingCause::Withdrawn)
                    }
                    QueueRemovalCause::SessionClosed => {
                        (InvocationStage::Cancelled, SchedulingCause::SessionClosed)
                    }
                    QueueRemovalCause::RunnerStopped => {
                        (InvocationStage::Cancelled, SchedulingCause::RunnerStopped)
                    }
                    QueueRemovalCause::DispatchFailed => {
                        (InvocationStage::Settled, SchedulingCause::DispatchFailed)
                    }
                };
                let explicit = matches!(
                    cause,
                    QueueRemovalCause::Withdrawn | QueueRemovalCause::SessionClosed
                );
                if edge.stage != expected_stage
                    || edge.cause != expected_cause
                    || entry.actor != edge.actor
                    || (entry.actor.is_some() != explicit)
                {
                    return Err(corrupt(
                        "queue removal disagrees with scheduling stage, cause, or caller",
                    ));
                }
            }
            QueueMutation::Reordered(change) => {
                reorders += 1;
                if reorders > QueueHistoryRecord::MAX_REORDERS
                    || change.is_unchanged()
                    || entry.actor.is_none()
                {
                    return Err(corrupt("invalid queue reorder evidence"));
                }
            }
            QueueMutation::Restored => {
                if entry.actor.is_some() {
                    return Err(corrupt(
                        "queue restoration cannot claim a caller cancellation",
                    ));
                }
            }
        }
        queue.apply_mutation(&entry.mutation).map_err(corrupt)?;
    }
    for (id, _) in queue.pending() {
        let record = records
            .get(&id)
            .expect("replayed membership references a submitted invocation");
        if record.scheduling.last().map(|edge| edge.stage) != Some(InvocationStage::Queued) {
            return Err(corrupt(
                "pending queue member has no matching pending scheduling state",
            ));
        }
    }
    for record in &snapshot.invocations {
        if record
            .scheduling
            .iter()
            .any(|edge| edge.stage == InvocationStage::Running)
            && !selected.contains(&record.request.execution_id)
        {
            return Err(corrupt("queued dispatch has no prior queue selection"));
        }
    }
    Ok(queue)
}
