//! Logical Nessa fact identity, separate from an event-stream cursor or retry ID.
//! One accepted fact can use one inline record or several sealed physical records.

pub(crate) mod continuation;

use super::{
    queue_validation::QueueReplayUndo,
    steering_position::SteeringPosition,
    validation::{InvocationContinuation, InvocationObservationUndo},
    SessionChange, SessionSnapshot, StorageError, SubmissionAcknowledgement,
};
use crate::{
    application::agent_execution::{
        agents::AgentError,
        executions::ExecutionUpdate,
        providers::{
            ModelChangeRecord, ProviderIdentity, SessionConfigurationKind,
            SessionConfigurationRecord,
        },
    },
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

/// The change a staged batch refused.
pub(crate) struct RefusedChange {
    /// The change `stage_change` refused. Absent when every change staged
    /// and a later check refused the batch.
    pub(crate) index: Option<usize>,
    pub(crate) error: StorageError,
}

impl RefusedChange {
    pub(crate) fn batch(error: StorageError) -> Self {
        Self { index: None, error }
    }
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

/// A steered message names a prior turn that can take steering. At admission
/// its offset is exactly the target's events so far, so it borrows no output
/// from the future
/// (`a_steering_offset_is_bounded_by_the_target_history_each_path_holds`). A
/// later injection takes no offset bound of its own; why is stated once, in
/// `docs/design/mcp-app-calls.md` ("The app a message names").
fn validate_target_prefix(
    snapshot: &SessionSnapshot,
    positions: &HashMap<ExecutionId, usize>,
    histories: &HashMap<ExecutionId, InvocationHistory>,
    steering: Option<SteeringPosition<'_>>,
    at_admission: bool,
) -> Result<(), StorageError> {
    let Some(steering) = steering else {
        return Ok(());
    };
    let target = steering.target();
    let record = positions
        .get(target)
        .and_then(|index| snapshot.invocations.get(*index))
        .ok_or_else(|| corrupt("steering target is not a prior invocation"))?;
    if at_admission && steering.offset() != record.events.len() {
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
    /// # Errors
    ///
    /// `Corrupt` for a half-saved steering position (`SteeringPosition::saved`).
    fn from_snapshot(snapshot: Option<&SessionSnapshot>) -> Result<Self, StorageError> {
        let Some(snapshot) = snapshot else {
            return Ok(Self::default());
        };
        Ok(Self {
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
            correlation: SteeringPosition::any_saved(&snapshot.invocations)?,
            selected: snapshot
                .queue_history
                .iter()
                .any(|record| matches!(record.mutation, QueueMutation::Selected { .. })),
        })
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
    continuation
        .stage(changes, &mut undo)
        .map_err(|refusal| refusal.error)?;
    continuation
        .snapshot
        .ok_or_else(|| corrupt("semantic batch has no session"))
}

pub(crate) fn affected_execution(change: &SessionChange) -> Option<&ExecutionId> {
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
    // Boxed: a tool observation carried inline made this variant larger than
    // the rest of the undo by enough to trip the size lint, once an MCP
    // call's arguments joined the observation.
    Observation(usize, Box<InvocationObservationUndo>),
    Receipt(usize, SubmissionAcknowledgement),
    Stop(usize),
    Report(usize),
    Settlement(
        usize,
        Option<ExecutionOutcome>,
        Option<Result<ExecutionOutcome, AgentError>>,
    ),
    Context(ProviderContext),
    Configuration(Option<SessionConfigurationRecord>),
    Model(ProviderIdentity, Option<ModelChangeRecord>),
    Provider(ProviderIdentity),
    /// Provider evidence before this change. Rollback of an abandoned unit
    /// restores it with the mutations; a later context record must not see
    /// evidence for an event that was rolled back.
    Evidence(ProviderEvidence),
    Lease(Option<super::CurrentLease>),
    Artifact,
    Note,
    NoteRetired(usize),
}

/// Units applied to a [`continuation::Continuation`] that its owner may still
/// need to take back: how to undo each of their changes, and the changes, to
/// apply them again. The owner decides how long they stay (the record writer
/// keeps a binding's units until the binding changes, so a retry can be
/// checked against the state before them), so the continuation can return to
/// an earlier state without a copy of it.
#[derive(Default)]
pub(crate) struct AppliedUnits {
    undo: Vec<ChangeUndo>,
    // Where each unit's undo begins in `undo`.
    starts: Vec<usize>,
    units: Vec<Vec<SessionChange>>,
}

impl continuation::Continuation {
    /// Apply one unit, and keep in `applied` how to undo it. A refused unit
    /// changes nothing.
    pub(crate) fn apply_kept(
        &mut self,
        changes: Vec<SessionChange>,
        applied: &mut AppliedUnits,
    ) -> Result<(), StorageError> {
        let mut undo = Vec::new();
        if let Err(refusal) = self.stage(&changes, &mut undo) {
            self.rollback(undo);
            return Err(refusal.error);
        }
        applied.starts.push(applied.undo.len());
        applied.undo.extend(undo);
        applied.units.push(changes);
        Ok(())
    }

    /// How many units `applied` holds.
    pub(crate) fn applied_len(applied: &AppliedUnits) -> usize {
        applied.units.len()
    }

    /// Return to the state before every unit in `applied`, and forget them.
    pub(crate) fn revert(&mut self, applied: &mut AppliedUnits) {
        self.revert_after(applied, 0);
    }

    /// Return to the state after the first `keep` units in `applied`, and
    /// forget the rest.
    pub(crate) fn revert_after(&mut self, applied: &mut AppliedUnits, keep: usize) {
        let Some(&start) = applied.starts.get(keep) else {
            return;
        };
        self.rollback(applied.undo.split_off(start));
        applied.starts.truncate(keep);
        applied.units.truncate(keep);
    }

    /// Run `read` on the state before the units in `applied`, then apply
    /// them again. Its cost is those units', not the history's.
    pub(crate) fn before_applied<T>(
        &mut self,
        applied: &mut AppliedUnits,
        read: impl FnOnce(&mut Self) -> T,
    ) -> Result<T, StorageError> {
        self.after_applied(applied, 0, read)
    }

    /// Run `read` on the state after only the first `keep` units in
    /// `applied`, then apply the rest again.
    pub(crate) fn after_applied<T>(
        &mut self,
        applied: &mut AppliedUnits,
        keep: usize,
        read: impl FnOnce(&mut Self) -> T,
    ) -> Result<T, StorageError> {
        let Some(&start) = applied.starts.get(keep) else {
            return Ok(read(self));
        };
        let units = applied.units.split_off(keep);
        applied.starts.truncate(keep);
        self.rollback(applied.undo.split_off(start));
        let value = read(self);
        for changes in units {
            // They applied to this same state before, so they apply again.
            self.apply_kept(changes, applied)
                .map_err(|error| corrupt(format!("applied units no longer apply: {error}")))?;
        }
        Ok(value)
    }

    /// Whether `units` apply in order to this state. It is left as it was.
    pub(crate) fn check_units(
        &mut self,
        units: &[super::SessionSaveUnit],
    ) -> Result<(), StorageError> {
        let mut undo = Vec::new();
        let mut result = Ok(());
        for unit in units {
            if let Err(refusal) = self.stage(unit.changes(), &mut undo) {
                result = Err(refusal.error);
                break;
            }
        }
        self.rollback(undo);
        result
    }
}

impl continuation::Continuation {
    pub(super) fn stage(
        &mut self,
        changes: &[SessionChange],
        undo: &mut Vec<ChangeUndo>,
    ) -> Result<(), RefusedChange> {
        for (index, change) in changes.iter().enumerate() {
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
            if let Err(error) = result {
                return Err(RefusedChange {
                    index: Some(index),
                    error,
                });
            }
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
            .ok_or_else(|| RefusedChange::batch(corrupt("semantic batch has no session")))?;
        for change in changes {
            if let Some(id) = affected_execution(change) {
                if let Some(history) = histories.get(id) {
                    history
                        .validate_checkpoint()
                        .map_err(|error| RefusedChange::batch(corrupt(error)))?;
                }
            }
        }
        if snapshot.queue_history.len()
            > super::QueueHistoryRecord::maximum_entries(snapshot.invocations.len())
        {
            return Err(RefusedChange::batch(corrupt(
                "queue history exceeds its structural bound",
            )));
        }
        queue
            .validate_current_checkpoint(snapshot, positions)
            .map_err(RefusedChange::batch)?;
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
                    model_change: None,
                    current_configuration: None,
                    id: id.clone(),
                    provider: provider.clone(),
                    provider_context: context.clone(),
                    invocations: Vec::new(),
                    queue_history: Vec::new(),
                    lease: None,
                    artifacts: Vec::new(),
                    notes: Vec::new(),
                });
            }
            SessionChange::InputAccepted(record) => {
                super::validation::validate_submission_acknowledgement(&record.acknowledgement)?;
                let mut state = InvocationContinuation::empty(record)?;
                let history = state.history.take().expect("new invocation history");
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("input precedes session open"))?;
                let steering = SteeringPosition::saved(record)?;
                validate_target_prefix(snapshot, positions, histories, steering, true)?;
                if snapshot.invocations.len() >= SessionSnapshot::MAX_INVOCATIONS {
                    return Err(corrupt("too many retained invocations"));
                }
                if positions.contains_key(&record.request.execution_id) {
                    return Err(corrupt("execution identity was accepted twice"));
                }
                // The target's observations end at the offset here
                // (`validate_target_prefix`), as they stood at admission.
                super::app_sources::validate_saved(
                    &record.request.user_message,
                    steering,
                    |execution, tool| {
                        positions.get(execution).and_then(|&index| {
                            invocations[index].mcp_tool(&snapshot.invocations[index], tool)
                        })
                    },
                )
                .map_err(corrupt)?;
                // Saving this record assigns its notes: each must be pending
                // as the records stood when it was saved.
                super::notes::admit_taken(snapshot, &record.notes)?;
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
                // New input carries one scheduling edge at most (above).
                next_evidence.correlation |= steering.is_some();
                next_evidence.dispatched |= record.scheduling.iter().any(|event| {
                    matches!(
                        event.stage,
                        InvocationStage::Running | InvocationStage::Injected
                    )
                });
                next_evidence.validate(&snapshot.provider_context)?;
                undo.push(ChangeUndo::Evidence(*provider_evidence));
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
                undo.push(ChangeUndo::Evidence(*provider_evidence));
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
                undo.push(ChangeUndo::Evidence(*provider_evidence));
                *provider_evidence = next_evidence;
                if event.stage == InvocationStage::Running && !queue.selected(execution_id) {
                    return Err(corrupt("queued dispatch has no prior queue selection"));
                }
                if event.stage == InvocationStage::Injected {
                    let record = positions
                        .get(execution_id)
                        .and_then(|index| snapshot.invocations.get(*index))
                        .ok_or_else(|| corrupt("semantic fact has no accepted input"))?;
                    // The admitted position still holds
                    // (`docs/design/mcp-app-calls.md`, "The app a message
                    // names"): an injection naming another target is refused
                    // below by `InvocationHistory::schedule`.
                    validate_target_prefix(
                        snapshot,
                        positions,
                        histories,
                        SteeringPosition::saved(record)?,
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
                undo.push(ChangeUndo::Evidence(*provider_evidence));
                *provider_evidence = next_evidence;
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
                let index = positions[event.execution_id()];
                let observation_undo =
                    invocations[index].observe(&context, record, record.events.len(), event)?;
                undo.push(ChangeUndo::Observation(index, Box::new(observation_undo)));
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
                undo.push(ChangeUndo::Evidence(*provider_evidence));
                *provider_evidence = next_evidence;
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
            SessionChange::ConfigurationChanged(record) => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("configuration change precedes session open"))?;
                if snapshot
                    .current_configuration
                    .as_ref()
                    .is_some_and(|current| Some(current.after()) != record.before())
                {
                    return Err(corrupt("configuration prior differs from current settings"));
                }
                if record.kind() == SessionConfigurationKind::Model {
                    return Err(corrupt("model configuration requires a model change"));
                }
                record
                    .validate_relationship(snapshot.model_change.as_ref())
                    .map_err(corrupt)?;
                undo.push(ChangeUndo::Configuration(
                    snapshot.current_configuration.replace(record.clone()),
                ));
            }
            SessionChange::ModelChanged(record) => {
                let change = record.change();
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("model change precedes session open"))?;
                if &snapshot.provider != change.before() {
                    return Err(StorageError::IdentityMismatch);
                }
                if snapshot
                    .current_configuration
                    .as_ref()
                    .is_some_and(|current| current.after() != change.prior_configuration())
                {
                    return Err(corrupt("model change prior differs from current settings"));
                }
                let configuration = SessionConfigurationRecord::new(
                    Some(snapshot.current_configuration.as_ref().map_or_else(
                        || record.change().prior_configuration().clone(),
                        |record| record.after().clone(),
                    )),
                    record.configuration().clone(),
                    record.actor().clone(),
                    SessionConfigurationKind::Model,
                );
                undo.push(ChangeUndo::Configuration(
                    snapshot.current_configuration.replace(configuration),
                ));
                undo.push(ChangeUndo::Model(
                    std::mem::replace(&mut snapshot.provider, change.after().clone()),
                    snapshot.model_change.replace(record.clone()),
                ));
            }
            SessionChange::ProviderIdentityChanged { before, after } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("provider identity precedes session open"))?;
                if &snapshot.provider != before {
                    return Err(StorageError::IdentityMismatch);
                }
                if before == after
                    || before.name() != after.name()
                    || before.model_id() != after.model_id()
                {
                    return Err(corrupt(
                        "provider identity change differs in more than its context",
                    ));
                }
                undo.push(ChangeUndo::Provider(std::mem::replace(
                    &mut snapshot.provider,
                    after.clone(),
                )));
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
                // Earlier observations are not checked against `after`: each
                // cancellation names the context in force at its own fact,
                // which was checked when that fact was staged.
                provider_evidence.validate(after)?;
                undo.push(ChangeUndo::Context(std::mem::replace(
                    &mut snapshot.provider_context,
                    after.clone(),
                )));
            }
            SessionChange::Lease(record) => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("lease record precedes session open"))?;
                let next = super::CurrentLease::apply(snapshot.lease.as_ref(), record)?;
                undo.push(ChangeUndo::Lease(snapshot.lease.replace(next)));
            }
            SessionChange::Artifact(record) => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("artifact record precedes session open"))?;
                let turn_actor = |turn: &ExecutionId| {
                    let index = *positions.get(turn)?;
                    Some(snapshot.invocations.get(index)?.actor.clone())
                };
                record.admit_saved(snapshot, turn_actor)?;
                snapshot.artifacts.push(record.clone());
                undo.push(ChangeUndo::Artifact);
            }
            SessionChange::Note(record) => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("note record precedes session open"))?;
                super::notes::admit_saved(snapshot, record)?;
                snapshot.notes.push(record.clone());
                undo.push(ChangeUndo::Note);
            }
            SessionChange::NoteRetired(record) => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("note retirement precedes session open"))?;
                let index = super::notes::admit_retired(snapshot, record)?;
                snapshot.notes[index].retired = Some(record.retirement);
                undo.push(ChangeUndo::NoteRetired(index));
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
                self.invocations[index].restore_observation(*undo);
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
            ChangeUndo::Model(provider, configuration) => {
                let snapshot = self.snapshot.as_mut().expect("open");
                snapshot.provider = provider;
                snapshot.model_change = configuration;
            }
            ChangeUndo::Provider(provider) => {
                self.snapshot.as_mut().expect("open").provider = provider;
            }
            ChangeUndo::Configuration(record) => {
                self.snapshot.as_mut().expect("open").current_configuration = record;
            }
            ChangeUndo::Context(context) => {
                self.snapshot.as_mut().expect("open").provider_context = context
            }
            ChangeUndo::Evidence(prior) => self.evidence = prior,
            ChangeUndo::Lease(prior) => {
                let snapshot = self.snapshot.as_mut().expect("open");
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(super::retained::lease(snapshot.lease.as_ref()))
                    .saturating_add(super::retained::lease(prior.as_ref()));
                snapshot.lease = prior;
            }
            ChangeUndo::Artifact => {
                let record = self
                    .snapshot
                    .as_mut()
                    .expect("open")
                    .artifacts
                    .pop()
                    .expect("artifact record");
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(super::retained::artifact(&record));
            }
            ChangeUndo::Note => {
                let record = self
                    .snapshot
                    .as_mut()
                    .expect("open")
                    .notes
                    .pop()
                    .expect("note record");
                self.snapshot_bytes = self
                    .snapshot_bytes
                    .saturating_sub(super::retained::note(&record));
            }
            ChangeUndo::NoteRetired(index) => {
                self.snapshot.as_mut().expect("open").notes[index].retired = None;
            }
        }
    }
}
