//! Logical Nessa fact identity, separate from an event-stream cursor or retry ID.
//! One accepted fact can use one inline record or several sealed physical records.
pub(crate) mod continuation;

use super::{
    queue_validation::QueueReplayUndo,
    validation::{InvocationContinuation, InvocationObservationUndo},
    SessionChange, SessionSnapshot, StorageError, SubmissionAcknowledgement,
};
use crate::{
    application::agent_execution::{agents::AgentError, executions::ExecutionUpdate},
    domain::agent_execution::{
        executions::{
            ExecutionId, ExecutionOutcome, InvocationHistory, InvocationObservation,
            InvocationStage, QueueMutation,
        },
        sessions::ProviderContext,
    },
};
use std::{collections::HashMap, fmt::Display};

/// Version 1 semantic fact kinds owned by the SDK conversation coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FactKind {
    SaveUnit,
    SaveComplete,
}

impl FactKind {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::SaveUnit => 12,
            Self::SaveComplete => 13,
        }
    }
    pub(crate) fn from_code(code: u8) -> Option<Self> {
        match code {
            12 => Some(Self::SaveUnit),
            13 => Some(Self::SaveComplete),
            _ => None,
        }
    }
}

/// Logical identity for exactly one fact in one conversation stream.
/// The stream incarnation scopes this key; its physical record IDs derive from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FactKey {
    kind: FactKind,
    execution_id: Option<ExecutionId>,
    ordinal: u64,
}

impl FactKey {
    /// Refuse a kind/identity combination that cannot name a fact.
    pub(crate) fn new(
        kind: FactKind,
        execution_id: Option<ExecutionId>,
        ordinal: u64,
    ) -> Option<Self> {
        if execution_id.is_some() {
            return None;
        }
        Some(Self {
            kind,
            execution_id,
            ordinal,
        })
    }

    pub(crate) fn kind(&self) -> FactKind {
        self.kind
    }

    pub(crate) fn execution_id(&self) -> Option<&ExecutionId> {
        self.execution_id.as_ref()
    }

    pub(crate) fn ordinal(&self) -> u64 {
        self.ordinal
    }
}

fn corrupt(message: impl Display) -> StorageError {
    StorageError::Corrupt(message.to_string())
}

fn invocation_at<'a>(
    snapshot: &'a mut SessionSnapshot,
    positions: &HashMap<ExecutionId, usize>,
    id: &ExecutionId,
) -> Result<&'a mut super::InvocationRecord, StorageError> {
    positions
        .get(id)
        .and_then(|index| snapshot.invocations.get_mut(*index))
        .ok_or_else(|| corrupt("semantic fact has no accepted input"))
}

/// Advance a cached domain authority without changing either cache or projection
/// when the proposed transition fails. Each affected history is rebuilt once.
fn apply_history<T>(
    histories: &mut HashMap<ExecutionId, InvocationHistory>,
    record: &super::InvocationRecord,
    transition: impl FnOnce(&mut InvocationHistory) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let id = &record.request.execution_id;
    let mut next = match histories.get(id) {
        Some(history) => history.clone(),
        None => return Err(corrupt("validated invocation history is absent")),
    };
    let result = transition(&mut next)?;
    histories.insert(id.clone(), next);
    Ok(result)
}

/// Admission captures the exact target output prefix. A later injection may
/// observe more output, but neither fact can borrow evidence from the future.
fn validate_target_prefix(
    snapshot: &SessionSnapshot,
    positions: &HashMap<ExecutionId, usize>,
    histories: &HashMap<ExecutionId, InvocationHistory>,
    target: Option<&ExecutionId>,
    offset: Option<usize>,
    exact: bool,
) -> Result<(), StorageError> {
    let Some(target) = target else {
        return offset
            .is_none()
            .then_some(())
            .ok_or_else(|| corrupt("targetless input has a steering offset"));
    };
    let record = positions
        .get(target)
        .and_then(|index| snapshot.invocations.get(*index))
        .ok_or_else(|| corrupt("steering target is not a prior invocation"))?;
    let count = record.events.len();
    if offset.is_none_or(|offset| {
        if exact {
            offset != count
        } else {
            offset > count
        }
    }) {
        return Err(corrupt(
            "steering offset is outside the prior target history",
        ));
    }
    if let Some(history) = histories.get(target) {
        return history
            .validate_steering_target()
            .map_err(|error| corrupt(error.to_string()));
    }
    Err(corrupt("validated target history is absent"))
}

#[derive(Clone, Copy, Default)]
pub(super) struct ProviderEvidence {
    observations: bool,
    reports: bool,
    dispatched: bool,
    correlation: bool,
    selected: bool,
}

impl ProviderEvidence {
    fn from_snapshot(snapshot: Option<&SessionSnapshot>) -> Self {
        let Some(snapshot) = snapshot else {
            return Self::default();
        };
        Self {
            observations: snapshot
                .invocations
                .iter()
                .any(|record| !record.events.is_empty()),
            reports: snapshot
                .invocations
                .iter()
                .any(|record| record.provider_report.is_some()),
            dispatched: snapshot.invocations.iter().any(|record| {
                record.scheduling.iter().any(|event| {
                    matches!(
                        event.stage,
                        InvocationStage::Running | InvocationStage::Injected
                    )
                })
            }),
            correlation: snapshot.invocations.iter().any(|record| {
                record.target_event_offset.is_some()
                    || record.scheduling.iter().any(|event| event.target.is_some())
            }),
            selected: snapshot
                .queue_history
                .iter()
                .any(|record| matches!(record.mutation, QueueMutation::Selected { .. })),
        }
    }

    fn validate(self, context: &ProviderContext) -> Result<(), StorageError> {
        context
            .validate_evidence(
                self.observations,
                self.reports,
                self.dispatched,
                self.correlation,
                self.selected,
            )
            .map_err(|error| StorageError::Corrupt(error.to_string()))
    }
}

/// Apply an ordered, atomic SDK decision batch to an already committed state.
/// The returned candidate is validated before a caller can publish its cursor.
/// Replay has no provider, audit or tool effects.
#[cfg(test)]
pub(crate) fn fold_changes(
    prior: Option<&SessionSnapshot>,
    changes: &[SessionChange],
) -> Result<SessionSnapshot, StorageError> {
    let mut continuation = continuation::Continuation::restore(prior.cloned())?;
    let mut undo = Vec::new();
    continuation.stage(changes, &mut undo)?;
    continuation
        .snapshot
        .ok_or_else(|| corrupt("semantic batch has no session"))
}

pub(super) fn affected_execution(change: &SessionChange) -> Option<&ExecutionId> {
    match change {
        SessionChange::InputAccepted(record) => Some(&record.request.execution_id),
        SessionChange::ProviderObservation(event) => Some(event.execution_id()),
        SessionChange::SchedulingTransition { execution_id, .. }
        | SessionChange::ReceiptUpdated { execution_id, .. }
        | SessionChange::StopDecision { execution_id, .. }
        | SessionChange::ProviderReport { execution_id, .. }
        | SessionChange::LocalSettlement { execution_id, .. } => Some(execution_id),
        _ => None,
    }
}

pub(super) struct AllocationUndo {
    id: Option<ExecutionId>,
}
pub(super) enum ChangeUndo {
    Allocation(AllocationUndo),
    History(ExecutionId, InvocationHistory),
    Opened,
    Input(ExecutionId),
    Queue(QueueReplayUndo),
    Scheduling(usize),
    Observation(usize, InvocationObservationUndo),
    Receipt(usize, SubmissionAcknowledgement),
    Stop(usize),
    Report(usize),
    Settlement(
        usize,
        Option<ExecutionOutcome>,
        Option<Result<ExecutionOutcome, AgentError>>,
    ),
    Context(ProviderContext),
}

impl continuation::Continuation {
    pub(super) fn stage(
        &mut self,
        changes: &[SessionChange],
        undo: &mut Vec<ChangeUndo>,
    ) -> Result<(), StorageError> {
        for change in changes {
            let before_snapshot =
                super::retained::touched(self.snapshot.as_ref(), change, &self.positions, false);
            let id = affected_execution(change).cloned();
            let before_derived = self.derived_touched(id.as_ref());
            undo.push(ChangeUndo::Allocation(AllocationUndo { id }));
            let result = self.stage_change(change, undo);
            let after_snapshot = super::retained::touched(
                self.snapshot.as_ref(),
                change,
                &self.positions,
                result.is_ok(),
            )
            .saturating_add(if result.is_ok() {
                super::retained::append_payload(change)
            } else {
                0
            });
            let after_derived = self.derived_touched(affected_execution(change));
            self.snapshot_bytes = self
                .snapshot_bytes
                .saturating_sub(before_snapshot)
                .saturating_add(after_snapshot);
            self.derived_bytes = self
                .derived_bytes
                .saturating_sub(before_derived)
                .saturating_add(after_derived);
            result?;
        }
        let Self {
            snapshot: candidate,
            histories,
            queue,
            positions,
            ..
        } = self;

        let snapshot = candidate
            .as_ref()
            .ok_or_else(|| corrupt("semantic batch has no session"))?;
        for change in changes {
            if let Some(id) = affected_execution(change) {
                if let Some(history) = histories.get(id) {
                    history.validate_checkpoint().map_err(corrupt)?;
                }
            }
        }
        if snapshot.queue_history.len()
            > super::QueueHistoryRecord::maximum_entries(snapshot.invocations.len())
        {
            return Err(corrupt("queue history exceeds its structural bound"));
        }
        queue.validate_current_checkpoint(snapshot, positions)?;
        Ok(())
    }

    fn stage_change(
        &mut self,
        change: &SessionChange,
        undo: &mut Vec<ChangeUndo>,
    ) -> Result<(), StorageError> {
        let Self {
            snapshot: candidate,
            positions,
            histories,
            invocations,
            queue,
            evidence: provider_evidence,
            context_witness,
            identity_bytes,
            ..
        } = self;

        if let Some(id) = affected_execution(change) {
            if let Some(history) = histories.get(id) {
                undo.push(ChangeUndo::History(id.clone(), history.clone()));
            }
        }
        match change {
            SessionChange::Opened {
                id,
                provider,
                context,
            } => {
                if candidate.is_some() {
                    return Err(corrupt("session was opened twice"));
                }
                provider_evidence.validate(context)?;
                undo.push(ChangeUndo::Opened);
                *candidate = Some(SessionSnapshot {
                    id: id.clone(),
                    provider: provider.clone(),
                    provider_context: context.clone(),
                    invocations: Vec::new(),
                    queue_history: Vec::new(),
                });
            }
            SessionChange::InputAccepted(record) => {
                super::validation::validate_submission_acknowledgement(&record.acknowledgement)?;
                let mut state = InvocationContinuation::empty(record)?;
                let history = state.history.take().expect("new invocation history");
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("input precedes session open"))?;
                validate_target_prefix(
                    snapshot,
                    positions,
                    histories,
                    record
                        .scheduling
                        .first()
                        .and_then(|event| event.target.as_ref()),
                    record.target_event_offset,
                    true,
                )?;
                if snapshot.invocations.len() >= SessionSnapshot::MAX_INVOCATIONS {
                    return Err(corrupt("too many retained invocations"));
                }
                if positions.contains_key(&record.request.execution_id) {
                    return Err(corrupt("execution identity was accepted twice"));
                }
                super::app_sources::validate_against(&record.request.user_message, |execution| {
                    positions
                        .get(execution)
                        .map(|&index| &snapshot.invocations[index])
                })
                .map_err(corrupt)?;
                if !record.events.is_empty()
                    || record.provider_report.is_some()
                    || record.local_cancellation.is_some()
                    || record.local_outcome.is_some()
                    || record.cancellation.is_some()
                    || record.result.is_some()
                    || record.scheduling.len() > 1
                {
                    return Err(corrupt("new input carries later evidence"));
                }
                let mut next_evidence = *provider_evidence;
                next_evidence.correlation |= record.target_event_offset.is_some()
                    || record.scheduling.iter().any(|event| event.target.is_some());
                next_evidence.dispatched |= record.scheduling.iter().any(|event| {
                    matches!(
                        event.stage,
                        InvocationStage::Running | InvocationStage::Injected
                    )
                });
                next_evidence.validate(&snapshot.provider_context)?;
                *provider_evidence = next_evidence;
                undo.push(ChangeUndo::Input(record.request.execution_id.clone()));
                *identity_bytes =
                    identity_bytes.saturating_add(2 * record.request.execution_id.as_str().len());
                invocations.push(state);
                positions.insert(
                    record.request.execution_id.clone(),
                    snapshot.invocations.len(),
                );
                histories.insert(record.request.execution_id.clone(), history);
                snapshot.invocations.push((**record).clone());
            }
            SessionChange::QueueDecision(decision) => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("queue decision precedes session open"))?;
                let mut next_evidence = *provider_evidence;
                next_evidence.selected |=
                    matches!(decision.mutation, QueueMutation::Selected { .. });
                next_evidence.validate(&snapshot.provider_context)?;
                let queue_undo = queue.apply_current(snapshot, positions, decision)?;
                undo.push(ChangeUndo::Queue(queue_undo));
                *provider_evidence = next_evidence;
                snapshot.queue_history.push(decision.clone());
            }
            SessionChange::SchedulingTransition {
                execution_id,
                event,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("scheduling precedes session open"))?;
                let mut next_evidence = *provider_evidence;
                next_evidence.dispatched |= matches!(
                    event.stage,
                    InvocationStage::Running | InvocationStage::Injected
                );
                next_evidence.correlation |= event.target.is_some();
                next_evidence.validate(&snapshot.provider_context)?;
                if event.stage == InvocationStage::Running && !queue.selected(execution_id) {
                    return Err(corrupt("queued dispatch has no prior queue selection"));
                }
                if event.stage == InvocationStage::Injected {
                    let record = positions
                        .get(execution_id)
                        .and_then(|index| snapshot.invocations.get(*index))
                        .ok_or_else(|| corrupt("semantic fact has no accepted input"))?;
                    validate_target_prefix(
                        snapshot,
                        positions,
                        histories,
                        event.target.as_ref(),
                        record.target_event_offset,
                        false,
                    )?;
                }
                let record = invocation_at(snapshot, positions, execution_id)?;
                if record.scheduling.is_empty() {
                    super::validation::validate_admission_actor(record, event)?;
                }
                apply_history(histories, record, |history| {
                    history
                        .schedule(
                            event
                                .transition()
                                .map_err(|error| corrupt(error.to_string()))?,
                        )
                        .map_err(|error| corrupt(error.to_string()))?;
                    super::validation::validate_stop_actor(
                        record.local_cancellation.as_ref(),
                        Some(event),
                    )
                })?;
                *provider_evidence = next_evidence;
                undo.push(ChangeUndo::Scheduling(positions[execution_id]));
                record.scheduling.push(event.clone());
            }
            SessionChange::ProviderObservation(event) => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("observation precedes session open"))?;
                let mut next_evidence = *provider_evidence;
                next_evidence.observations = true;
                next_evidence.validate(&snapshot.provider_context)?;
                super::validation::validate_observation_context(&snapshot.provider_context, event)?;
                let context = snapshot.provider_context.clone();
                let record = invocation_at(snapshot, positions, event.execution_id())?;
                event
                    .validate_payload_size()
                    .map_err(|error| corrupt(error.to_string()))?;
                let observation = match event.update() {
                    ExecutionUpdate::Finished(outcome) => InvocationObservation::Finished(*outcome),
                    ExecutionUpdate::PermissionCancelled(_) => {
                        InvocationObservation::PermissionCancellation
                    }
                    _ => InvocationObservation::Output,
                };
                apply_history(histories, record, |history| {
                    history
                        .observe(event.execution_id(), observation)
                        .map_err(|error| corrupt(error.to_string()))
                })?;
                *provider_evidence = next_evidence;
                let index = positions[event.execution_id()];
                let observation_undo =
                    invocations[index].observe(&context, record, record.events.len(), event)?;
                undo.push(ChangeUndo::Observation(index, observation_undo));
                if context_witness.is_none()
                    && matches!(event.update(), ExecutionUpdate::PermissionCancelled(_))
                {
                    *context_witness = Some((index, record.events.len()));
                }
                crate::application::agent_execution::executions::limits::reserve_observation_slot(
                    &mut record.events,
                );
                record.events.push(event.clone());
            }
            SessionChange::ReceiptUpdated {
                execution_id,
                before,
                after,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("receipt precedes session open"))?;
                let record = invocation_at(snapshot, positions, execution_id)?;
                if &record.acknowledgement != before || before == after {
                    return Err(corrupt("receipt revision does not match prior value"));
                }
                super::validation::validate_submission_acknowledgement(after)?;
                undo.push(ChangeUndo::Receipt(
                    positions[execution_id],
                    std::mem::replace(&mut record.acknowledgement, after.clone()),
                ));
            }
            SessionChange::StopDecision {
                execution_id,
                event,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("stop precedes session open"))?;
                let record = invocation_at(snapshot, positions, execution_id)?;
                if record.cancellation.is_some() {
                    return Err(corrupt("undispatched stop was already recorded"));
                }
                apply_history(histories, record, |history| {
                    history
                        .record_cancellation(event.cancellation()?)
                        .map_err(|error| corrupt(error.to_string()))
                })?;
                undo.push(ChangeUndo::Stop(positions[execution_id]));
                record.cancellation = Some(event.clone());
            }
            SessionChange::ProviderReport {
                execution_id,
                report,
                local_stop,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("provider report precedes session open"))?;
                let mut next_evidence = *provider_evidence;
                next_evidence.reports = true;
                next_evidence.validate(&snapshot.provider_context)?;
                let record = invocation_at(snapshot, positions, execution_id)?;
                if record.provider_report.is_some() || record.local_cancellation.is_some() {
                    return Err(corrupt("provider report was already recorded"));
                }
                if let Some(result) = &record.result {
                    super::validation::validate_report_against_local_result(report, result)?;
                }
                apply_history(histories, record, |history| {
                    super::validation::record_report(
                        history,
                        Some(report),
                        local_stop.as_ref(),
                        record.scheduling.last(),
                    )
                })?;
                *provider_evidence = next_evidence;
                undo.push(ChangeUndo::Report(positions[execution_id]));
                record.provider_report = Some(report.clone());
                record.local_cancellation = local_stop.clone();
            }
            SessionChange::LocalSettlement {
                execution_id,
                before,
                after,
                local_outcome,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("local result precedes session open"))?;
                let record = invocation_at(snapshot, positions, execution_id)?;
                if &record.result != before || record.result.as_ref() == Some(after) {
                    return Err(corrupt("local result revision does not match prior value"));
                }
                super::validation::validate_local_result(record, after)?;
                let retained = apply_history(histories, record, |history| {
                    history
                        .record_local_result(after.as_ref().copied().map_err(|_| ()))
                        .map_err(|error| corrupt(error.to_string()))?;
                    Ok(history.local_outcome())
                })?;
                if retained != *local_outcome {
                    return Err(corrupt("local result changes its retained outcome"));
                }
                undo.push(ChangeUndo::Settlement(
                    positions[execution_id],
                    record.local_outcome,
                    record.result.replace(after.clone()),
                ));
                record.local_outcome = *local_outcome;
            }
            SessionChange::ProviderContext { before, after } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("provider context precedes session open"))?;
                if &snapshot.provider_context != before || before == after {
                    return Err(corrupt(
                        "provider context revision does not match prior value",
                    ));
                }
                provider_evidence.validate(after)?;
                if let Some((invocation, event)) = *context_witness {
                    super::validation::validate_observation_context(
                        after,
                        &snapshot.invocations[invocation].events[event],
                    )?;
                }
                undo.push(ChangeUndo::Context(std::mem::replace(
                    &mut snapshot.provider_context,
                    after.clone(),
                )));
            }
        }

        Ok(())
    }

    pub(super) fn rollback(&mut self, mut undo: Vec<ChangeUndo>) {
        if undo.is_empty() {
            return;
        }
        let snapshot_global = super::retained::snapshot_global(self.snapshot.as_ref());
        self.snapshot_bytes = self.snapshot_bytes.saturating_sub(snapshot_global);
        let global = self.derived_global();
        self.derived_bytes = self.derived_bytes.saturating_sub(global);
        // stage places an allocation marker before its mutation tokens. Reverse
        // search visits only this last group; popping it consumes that suffix.
        while let Some(index) = undo
            .iter()
            .rposition(|change| matches!(change, ChangeUndo::Allocation(_)))
        {
            let ChangeUndo::Allocation(accounting) = &undo[index] else {
                unreachable!("located allocation marker")
            };
            let before_owner = self.derived_owner(accounting.id.as_ref());
            while undo.len() > index + 1 {
                self.rollback_change(undo.pop().expect("group mutation"));
            }
            let ChangeUndo::Allocation(accounting) = undo.last().expect("group allocation") else {
                unreachable!("allocation marker precedes group mutations")
            };
            let after_owner = self.derived_owner(accounting.id.as_ref());
            self.rollback_change(undo.pop().expect("group allocation"));
            self.derived_bytes = self
                .derived_bytes
                .saturating_sub(before_owner)
                .saturating_add(after_owner);
        }
        assert!(undo.is_empty(), "stage records allocation before mutation");
        // Surviving global spare capacity is reconciled once for the whole batch.
        self.derived_bytes = self.derived_bytes.saturating_add(self.derived_global());
        self.snapshot_bytes = self
            .snapshot_bytes
            .saturating_add(super::retained::snapshot_global(self.snapshot.as_ref()));
    }

    fn rollback_change(&mut self, undo: ChangeUndo) {
        match undo {
            ChangeUndo::Allocation(_) => {}
            ChangeUndo::History(id, history) => {
                self.histories.insert(id, history);
            }
            ChangeUndo::Opened => {
                self.snapshot = None;
                self.snapshot_bytes = 0;
            }
            ChangeUndo::Input(id) => {
                self.identity_bytes = self.identity_bytes.saturating_sub(2 * id.as_str().len());
                self.positions.remove(&id);
                self.histories.remove(&id);
                self.invocations.pop();
                let record = self
                    .snapshot
                    .as_mut()
                    .expect("open")
                    .invocations
                    .pop()
                    .expect("new input");
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(super::retained::invocation(&record));
            }
            ChangeUndo::Queue(undo) => {
                self.queue.restore(undo);
                let entry = self
                    .snapshot
                    .as_mut()
                    .expect("open")
                    .queue_history
                    .pop()
                    .expect("queue decision");
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(super::retained::queue_entry(&entry));
            }
            ChangeUndo::Scheduling(index) => {
                let event = self.snapshot.as_mut().expect("open").invocations[index]
                    .scheduling
                    .pop()
                    .expect("scheduling transition");
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(super::retained::scheduling_payload(&event));
            }
            ChangeUndo::Observation(index, undo) => {
                self.invocations[index].restore_observation(undo);
                let event = self.snapshot.as_mut().expect("open").invocations[index]
                    .events
                    .pop()
                    .expect("observation");
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(super::retained::observation_payload(&event));
            }
            ChangeUndo::Receipt(index, receipt) => {
                let record = &mut self.snapshot.as_mut().expect("open").invocations[index];
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(super::retained::acknowledgement(&record.acknowledgement))
                    .saturating_add(super::retained::acknowledgement(&receipt));
                record.acknowledgement = receipt;
            }
            ChangeUndo::Stop(index) => {
                let record = &mut self.snapshot.as_mut().expect("open").invocations[index];
                self.snapshot_bytes = self.snapshot_bytes.saturating_sub(
                    record
                        .cancellation
                        .as_ref()
                        .map_or(0, super::retained::stop),
                );
                record.cancellation = None;
            }
            ChangeUndo::Report(index) => {
                let record = &mut self.snapshot.as_mut().expect("open").invocations[index];
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(
                        record
                            .provider_report
                            .as_ref()
                            .map_or(0, |report| report.retained_bytes()),
                    )
                    .saturating_sub(
                        record
                            .local_cancellation
                            .as_ref()
                            .map_or(0, super::retained::stop),
                    );
                record.provider_report = None;
                record.local_cancellation = None;
            }
            ChangeUndo::Settlement(index, outcome, result) => {
                let record = &mut self.snapshot.as_mut().expect("open").invocations[index];
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(super::retained::result_payload(record.result.as_ref()))
                    .saturating_add(super::retained::result_payload(result.as_ref()));
                record.result = result;
                record.local_outcome = outcome;
            }
            ChangeUndo::Context(context) => {
                self.snapshot.as_mut().expect("open").provider_context = context
            }
        }
    }
}
