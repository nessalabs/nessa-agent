//! Canonical owned semantic continuation and derived validated authorities.
use super::ProviderEvidence;
use crate::application::agent_execution::executions::ExecutionUpdate;
use crate::application::agent_execution::sessions::{
    queue_validation::QueueReplay,
    retained,
    validation::{self, InvocationContinuation},
    SessionChange, SessionSnapshot, StorageError,
};
use crate::domain::agent_execution::executions::{ExecutionId, InvocationHistory};
use std::{collections::HashMap, mem::size_of};

pub(crate) struct Continuation {
    pub(crate) snapshot: Option<SessionSnapshot>,
    pub(crate) positions: HashMap<ExecutionId, usize>,
    pub(crate) histories: HashMap<ExecutionId, InvocationHistory>,
    pub(in crate::application::agent_execution::sessions) invocations: Vec<InvocationContinuation>,
    pub(in crate::application::agent_execution::sessions) queue: QueueReplay,
    pub(in crate::application::agent_execution::sessions) evidence: ProviderEvidence,
    pub(crate) context_witness: Option<(usize, usize)>,
    pub(crate) snapshot_bytes: usize,
    pub(crate) derived_bytes: usize,
    pub(crate) identity_bytes: usize,
}
impl Continuation {
    /// Validate one complete caller-selected checkpoint without rebuilding prior history.
    pub(crate) fn apply_unit(&mut self, changes: &[SessionChange]) -> Result<(), StorageError> {
        let mut undo = Vec::new();
        if let Err(error) = self.stage(changes, &mut undo) {
            self.rollback(undo);
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn empty() -> Self {
        let mut value = Self {
            snapshot: None,
            positions: HashMap::new(),
            histories: HashMap::new(),
            invocations: Vec::new(),
            queue: QueueReplay::empty(),
            evidence: ProviderEvidence::default(),
            context_witness: None,
            snapshot_bytes: 0,
            derived_bytes: 0,
            identity_bytes: 0,
        };
        value.derived_bytes = value.derived_global();
        value
    }
    pub(crate) fn restore(snapshot: Option<SessionSnapshot>) -> Result<Self, StorageError> {
        let Some(snapshot) = snapshot else {
            return Ok(Self::empty());
        };
        let mut invocations = validation::continuation(&snapshot)?;
        let mut positions = HashMap::with_capacity(snapshot.invocations.len());
        let mut histories = HashMap::with_capacity(snapshot.invocations.len());
        for (index, (record, state)) in snapshot
            .invocations
            .iter()
            .zip(&mut invocations)
            .enumerate()
        {
            positions.insert(record.request.execution_id.clone(), index);
            histories.insert(
                record.request.execution_id.clone(),
                state.history.take().expect("rebuilt history"),
            );
        }
        let context_witness =
            snapshot
                .invocations
                .iter()
                .enumerate()
                .find_map(|(index, record)| {
                    record
                        .events
                        .iter()
                        .position(|event| {
                            matches!(event.update(), ExecutionUpdate::PermissionCancelled(_))
                        })
                        .map(|event| (index, event))
                });
        let queue = QueueReplay::from_snapshot(&snapshot)?;
        let evidence = ProviderEvidence::from_snapshot(Some(&snapshot));
        let snapshot_bytes = retained::snapshot(&snapshot);
        let identity_bytes = snapshot
            .invocations
            .iter()
            .map(|record| 2 * record.request.execution_id.as_str().len())
            .sum();
        let derived_bytes = 0;
        let mut continuation = Self {
            snapshot: Some(snapshot),
            positions,
            histories,
            invocations,
            queue,
            evidence,
            context_witness,
            snapshot_bytes,
            derived_bytes,
            identity_bytes,
        };
        continuation.derived_bytes = continuation
            .derived_global()
            .saturating_add(
                continuation
                    .invocations
                    .iter()
                    .map(InvocationContinuation::retained_bytes)
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(
                continuation
                    .histories
                    .values()
                    .map(InvocationHistory::allocation_bytes)
                    .fold(0usize, usize::saturating_add),
            );
        Ok(continuation)
    }
}

impl Default for Continuation {
    fn default() -> Self {
        Self::empty()
    }
}

impl Continuation {
    pub(crate) fn derived_global(&self) -> usize {
        0usize
            .saturating_add(
                self.positions
                    .capacity()
                    .saturating_mul(size_of::<(ExecutionId, usize)>() + size_of::<usize>()),
            )
            .saturating_add(
                self.histories.capacity().saturating_mul(
                    size_of::<(ExecutionId, InvocationHistory)>() + size_of::<usize>(),
                ),
            )
            .saturating_add(
                self.invocations
                    .capacity()
                    .saturating_mul(size_of::<InvocationContinuation>()),
            )
            .saturating_add(self.identity_bytes)
            .saturating_add(self.queue.retained_bytes())
    }
    pub(super) fn derived_touched(&self, id: Option<&ExecutionId>) -> usize {
        self.derived_global().saturating_add(self.derived_owner(id))
    }
    pub(super) fn derived_owner(&self, id: Option<&ExecutionId>) -> usize {
        0usize
            .saturating_add(
                id.and_then(|id| self.positions.get(id))
                    .and_then(|index| self.invocations.get(*index))
                    .map_or(0, InvocationContinuation::retained_bytes),
            )
            .saturating_add(
                id.and_then(|id| self.histories.get(id))
                    .map_or(0, InvocationHistory::allocation_bytes),
            )
    }
}
