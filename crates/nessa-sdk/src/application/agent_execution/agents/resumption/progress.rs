//! A committed resumption frame for readers waiting on a turn's final outcome.
use super::{ended_for_good, ending, lineage::Lineage, owed, Agent};
use crate::application::agent_execution::{
    caller_wake::{contain_caller_wake, CallerWaiter},
    sessions::{InvocationRecord, SessionSnapshot},
};
use crate::domain::agent_execution::executions::ExecutionId;
use std::{collections::HashSet, sync::Arc};

/// Immutable committed history and the work that same history still owes.
///
/// The frame retains snapshot data only, without retaining its Agent, writer
/// lease or provider. Owed work uses the SDK's resumption policy, including
/// known provider outcomes beneath later failures. Unacknowledged successor or
/// result observations cannot make a committed predecessor final in this frame.
/// [`Self::has_observed`] also distinguishes a missing committed record from a
/// turn this process has observed while its save remains unacknowledged.
pub struct ResumptionHistory {
    snapshot: Arc<SessionSnapshot>,
    owed: HashSet<ExecutionId>,
    finished: HashSet<ExecutionId>,
    observed: HashSet<ExecutionId>,
    lineage: Lineage,
}

impl ResumptionHistory {
    /// Classify one committed snapshot of a conversation with no live Agent.
    /// No execution is treated as owned by the current process. The same SDK
    /// policy used by [`Agent::resumption_history`] decides which turns are owed.
    pub fn from_committed(snapshot: Arc<SessionSnapshot>) -> Self {
        let observed = snapshot
            .invocations
            .iter()
            .map(|record| record.request.execution_id.clone())
            .collect();
        Self::from_frame(snapshot, observed, |_| false)
    }

    /// Invocation records borrowed from this immutable committed frame.
    pub fn invocations(&self) -> &[InvocationRecord] {
        &self.snapshot.invocations
    }

    /// A successor bound to an earlier stopped turn by matching its expected
    /// request, caller and initiation in this committed frame. The frame owns
    /// the returned identity; an unrelated record with a derived-looking ID
    /// establishes no successor binding.
    pub fn successor_of(&self, execution: &ExecutionId) -> Option<&ExecutionId> {
        self.lineage.successor_of(execution)
    }

    /// Whether this frame's committed history still owes `execution`.
    pub fn is_owed(&self, execution: &ExecutionId) -> bool {
        self.owed.contains(execution)
    }

    /// Whether this leaf has a final local ending in the committed frame,
    /// including a saved result prefix whose final scheduling edge was not
    /// recorded before process loss. Resolve injection hosts and authenticated
    /// successors first: an execution with a successor is not a finished leaf.
    /// Active prefixes owned by this Agent and leaves still owing work are not
    /// finished. This read changes no record and grants no execution authority.
    pub fn is_finished(&self, execution: &ExecutionId) -> bool {
        self.finished.contains(execution)
    }

    /// Whether the captured local history has observed `execution`, including
    /// a record whose save has not been acknowledged. For a frame constructed
    /// with [`Self::from_committed`], only committed identities are observed.
    pub fn has_observed(&self, execution: &ExecutionId) -> bool {
        self.observed.contains(execution)
    }

    fn from_frame(
        snapshot: Arc<SessionSnapshot>,
        observed: HashSet<ExecutionId>,
        live: impl Fn(&ExecutionId) -> bool,
    ) -> Self {
        let lineage = Lineage::new(&snapshot.invocations);
        let owed: HashSet<ExecutionId> = {
            let records = &snapshot.invocations;
            let newest_final = records
                .iter()
                .rposition(|record| ended_for_good(record, &lineage, &live));
            records
                .iter()
                .enumerate()
                .filter_map(|(index, record)| {
                    owed(record, &lineage, newest_final > Some(index), &live)
                        .map(|_| record.request.execution_id.clone())
                })
                .collect()
        };
        let finished = snapshot
            .invocations
            .iter()
            .filter_map(|record| {
                let id = &record.request.execution_id;
                (lineage.successor_of(id).is_none()
                    && !owed.contains(id)
                    && ending(record, live(id)).is_some())
                .then(|| id.clone())
            })
            .collect();
        Self {
            snapshot,
            owed,
            finished,
            observed,
            lineage,
        }
    }
}

impl Agent {
    /// Capture committed resumption history with this Agent's live ownership.
    /// Returns `None` before a committed snapshot exists. The committed snapshot
    /// and locally observed identities are captured under one evidence lock,
    /// while the scheduler lock keeps submission ownership stable. No turn is
    /// admitted, resumed or changed by this read.
    ///
    /// The frame shares its immutable snapshot. Its lineage index uses linear
    /// identity storage and compares candidate successor inputs with the SDK's
    /// bounded request construction; it does not copy the history. Caller-waker panics
    /// are contained while waiting for the scheduler or history evidence.
    pub async fn resumption_history(&self) -> Option<ResumptionHistory> {
        contain_caller_wake(
            CallerWaiter::ResumptionState(self.inner.manager.id().clone()),
            async {
                let scheduler = self.inner.scheduler.lock().await;
                let active = self.inner.lifecycle.active();
                let (committed, observed) = self.inner.manager.resumption_evidence().await?;
                Some(ResumptionHistory::from_frame(committed, observed, |id| {
                    scheduler.has_receipt(id) || active.as_ref() == Some(id)
                }))
            },
        )
        .await
    }
}
