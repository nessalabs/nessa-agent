//! Replay actual scheduler membership independently of later provider-dispatch edges.
use super::{QueueHistoryRecord, SessionSnapshot, StorageError};
use crate::application::agent_execution::executions::limits::validate_observation_id;
use crate::domain::agent_execution::executions::{
    ExecutionId, InvocationQueue, InvocationStage, QueueMutation, QueueOrderChange,
    QueueRemovalCause, SchedulingCause,
};
use std::collections::{HashMap, HashSet};
fn corrupt(message: &str) -> StorageError {
    StorageError::Corrupt(message.into())
}
pub(super) fn replay(snapshot: &SessionSnapshot) -> Result<InvocationQueue, StorageError> {
    Ok(QueueReplay::from_snapshot(snapshot)?.queue)
}

/// The same queue authority used by restoration and ordered semantic folding.
/// Its mutations remain local until the whole candidate is validated and saved.
pub(super) struct QueueReplay {
    queue: InvocationQueue,
    selected: HashSet<ExecutionId>,
    reorders: usize,
}

impl QueueReplay {
    pub(super) fn empty() -> Self {
        Self {
            queue: InvocationQueue::new(QueueOrderChange::MAX_PENDING)
                .expect("positive queue capacity"),
            selected: HashSet::new(),
            reorders: 0,
        }
    }

    pub(super) fn from_snapshot(snapshot: &SessionSnapshot) -> Result<Self, StorageError> {
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
        let positions: HashMap<_, _> = snapshot
            .invocations
            .iter()
            .enumerate()
            .map(|(index, record)| (record.request.execution_id.clone(), index))
            .collect();
        let mut state = Self::empty();
        for entry in &snapshot.queue_history {
            state.apply(snapshot, &positions, entry, false)?;
        }
        state.validate_checkpoint(snapshot)?;
        Ok(state)
    }

    /// Validate and apply one decision against the queue and scheduling prefix
    /// that existed when this fact was recorded.
    pub(super) fn apply_current(
        &mut self,
        snapshot: &SessionSnapshot,
        positions: &HashMap<ExecutionId, usize>,
        entry: &QueueHistoryRecord,
    ) -> Result<(), StorageError> {
        self.apply(snapshot, positions, entry, true)
    }

    fn apply(
        &mut self,
        snapshot: &SessionSnapshot,
        positions: &HashMap<ExecutionId, usize>,
        entry: &QueueHistoryRecord,
        require_current_checkpoint: bool,
    ) -> Result<(), StorageError> {
        let checkpoint = if let Some(id) = entry.mutation.id() {
            validate_observation_id(id.as_str())
                .map_err(|_| corrupt("queue identity exceeds its bound"))?;
            let record = positions
                .get(id)
                .and_then(|index| snapshot.invocations.get(*index))
                .ok_or_else(|| corrupt("queue event has no submitted invocation"))?;
            if require_current_checkpoint
                && entry.scheduling_length != Some(record.scheduling.len())
            {
                return Err(corrupt(
                    "queue decision names an earlier or future scheduling checkpoint",
                ));
            }
            let index = entry
                .scheduling_length
                .and_then(|length| length.checked_sub(1))
                .ok_or_else(|| corrupt("queue event has no scheduling checkpoint"))?;
            Some((
                record,
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
            QueueMutation::Selected { .. } => {
                let (_, edge) = checkpoint.expect("single-input event");
                if edge.stage != InvocationStage::Queued || entry.actor.is_some() {
                    return Err(corrupt("queue selection disagrees with pending checkpoint"));
                }
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
                if self.reorders >= QueueHistoryRecord::MAX_REORDERS
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
        self.queue
            .apply_mutation(&entry.mutation)
            .map_err(corrupt)?;
        if let QueueMutation::Selected { id } = &entry.mutation {
            self.selected.insert(id.clone());
        }
        if matches!(&entry.mutation, QueueMutation::Reordered(_)) {
            self.reorders += 1;
        }
        Ok(())
    }

    /// A running queued invocation must have been selected before its dispatch
    /// fact, even when a later queue decision would make the final snapshot valid.
    pub(super) fn selected(&self, id: &ExecutionId) -> bool {
        self.selected.contains(id)
    }

    fn validate_checkpoint(&self, snapshot: &SessionSnapshot) -> Result<(), StorageError> {
        let positions: HashMap<_, _> = snapshot
            .invocations
            .iter()
            .enumerate()
            .map(|(index, record)| (record.request.execution_id.clone(), index))
            .collect();
        for (id, _) in self.queue.pending() {
            let record = positions
                .get(&id)
                .and_then(|index| snapshot.invocations.get(*index))
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
                && !self.selected.contains(&record.request.execution_id)
            {
                return Err(corrupt("queued dispatch has no prior queue selection"));
            }
        }
        Ok(())
    }
}
