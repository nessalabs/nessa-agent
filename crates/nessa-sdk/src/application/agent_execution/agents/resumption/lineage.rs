//! Authenticated successor bindings within one ordered invocation frame.
use super::{dispatched, owed, successor_request};
use crate::application::agent_execution::{
    executions::ExecutionRequest, sessions::InvocationRecord,
};
use crate::domain::agent_execution::executions::{
    ExecutionId, InvocationStage, Resumption, SubmissionMode,
};
use std::collections::HashMap;

struct Origin {
    index: usize,
    root: usize,
    resumption: Resumption,
    dispatched: bool,
}

/// Indexes identities once; only matching input and caller facts bind a later
/// record to an earlier stopped predecessor. Other identities remain opaque.
pub(in crate::application::agent_execution::agents) struct Lineage {
    origins: HashMap<ExecutionId, Origin>,
    successors: HashMap<ExecutionId, ExecutionId>,
}

pub(super) struct Continuation {
    pub(super) original: ExecutionRequest,
    pub(super) resumption: Resumption,
    pub(super) dispatched: bool,
}

impl Lineage {
    pub(in crate::application::agent_execution::agents) fn new(
        records: &[InvocationRecord],
    ) -> Self {
        let mut lineage = Self {
            origins: HashMap::new(),
            successors: HashMap::new(),
        };
        let mut expected: HashMap<ExecutionId, usize> = HashMap::new();
        for (index, record) in records.iter().enumerate() {
            let id = &record.request.execution_id;
            let bound = expected.remove(id).and_then(|previous| {
                let predecessor = &records[previous];
                let origin = &lineage.origins[&predecessor.request.execution_id];
                let cause = predecessor.scheduling.last()?.cause;
                let request = successor_request(
                    &records[origin.root].request,
                    origin.dispatched,
                    cause,
                    id.clone(),
                )
                .ok()?;
                (record.submission == SubmissionMode::Queued
                    && record.request == request
                    && record.actor == predecessor.actor
                    && record.initiation == predecessor.initiation)
                    .then_some((previous, origin.root, origin.dispatched))
            });
            let origin = match bound {
                Some((previous, root, reached)) => {
                    lineage
                        .successors
                        .insert(records[previous].request.execution_id.clone(), id.clone());
                    Origin {
                        index,
                        root,
                        resumption: Resumption::of(id),
                        dispatched: reached || dispatched(record),
                    }
                }
                None => Origin {
                    index,
                    root: index,
                    resumption: Resumption::original(id),
                    dispatched: dispatched(record),
                },
            };
            if record.submission != SubmissionMode::Immediate
                && record.scheduling.last().is_some_and(|edge| {
                    matches!(
                        edge.stage,
                        InvocationStage::Cancelled | InvocationStage::Settled
                    ) && owed(record, &lineage, false, |_| false).is_some()
                })
            {
                if let Some(next) = origin.resumption.successor() {
                    expected.insert(next, index);
                }
            }
            lineage.origins.insert(id.clone(), origin);
        }
        lineage
    }

    pub(in crate::application::agent_execution::agents) fn successor_of(
        &self,
        id: &ExecutionId,
    ) -> Option<&ExecutionId> {
        self.successors.get(id)
    }

    pub(in crate::application::agent_execution::agents) fn record<'a>(
        &self,
        records: &'a [InvocationRecord],
        id: &ExecutionId,
    ) -> Option<&'a InvocationRecord> {
        self.origins.get(id).map(|origin| &records[origin.index])
    }

    pub(super) fn is_successor(&self, id: &ExecutionId) -> bool {
        self.origins
            .get(id)
            .is_some_and(|origin| origin.resumption.attempt() > 0)
    }

    pub(super) fn continuation(
        &self,
        records: &[InvocationRecord],
        id: &ExecutionId,
    ) -> Continuation {
        let origin = &self.origins[id];
        Continuation {
            original: records[origin.root].request.clone(),
            resumption: origin.resumption.clone(),
            dispatched: origin.dispatched,
        }
    }
}
