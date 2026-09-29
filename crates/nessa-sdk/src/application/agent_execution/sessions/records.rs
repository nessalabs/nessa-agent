//! Logical Nessa fact identity, separate from an event-stream cursor or retry ID.
//! One accepted fact can use one inline record or several sealed physical records.

use crate::domain::agent_execution::executions::ExecutionId;

/// Version 1 semantic fact kinds owned by the SDK conversation coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FactKind {
    SessionOpen,
    InputAccepted,
    QueueDecision,
    SchedulingTransition,
    ProviderObservation,
    ReceiptUpdated,
    StopDecision,
    ProviderReport,
    LocalSettlement,
    ProviderContext,
    AtomicTransition,
}

impl FactKind {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::SessionOpen => 1,
            Self::InputAccepted => 2,
            Self::QueueDecision => 3,
            Self::SchedulingTransition => 4,
            Self::ProviderObservation => 5,
            Self::ReceiptUpdated => 6,
            Self::StopDecision => 7,
            Self::ProviderReport => 8,
            Self::LocalSettlement => 9,
            Self::ProviderContext => 10,
            Self::AtomicTransition => 11,
        }
    }

    pub(crate) fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::SessionOpen,
            2 => Self::InputAccepted,
            3 => Self::QueueDecision,
            4 => Self::SchedulingTransition,
            5 => Self::ProviderObservation,
            6 => Self::ReceiptUpdated,
            7 => Self::StopDecision,
            8 => Self::ProviderReport,
            9 => Self::LocalSettlement,
            10 => Self::ProviderContext,
            11 => Self::AtomicTransition,
            _ => return None,
        })
    }

    fn needs_execution(self) -> bool {
        matches!(
            self,
            Self::InputAccepted
                | Self::SchedulingTransition
                | Self::ProviderObservation
                | Self::ReceiptUpdated
                | Self::StopDecision
                | Self::ProviderReport
                | Self::LocalSettlement
        )
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
        if kind.needs_execution() != execution_id.is_some()
            || (matches!(
                kind,
                FactKind::SessionOpen
                    | FactKind::InputAccepted
                    | FactKind::StopDecision
                    | FactKind::ProviderReport
            ) && ordinal != 0)
        {
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

/// Derive a retry-stable key from the last committed state. For revisions whose
/// count is not retained in a snapshot, the previous physical cursor is their
/// ordinal. A failed append cannot advance that cursor.
pub(crate) fn key_for_changes(
    prior: Option<&SessionSnapshot>,
    changes: &[SessionChange],
    previous_offset: u64,
) -> Result<FactKey, StorageError> {
    let [change] = changes else {
        if changes.len() < 2 {
            return Err(corrupt("empty semantic batch"));
        }
        return FactKey::new(FactKind::AtomicTransition, None, previous_offset)
            .ok_or_else(|| corrupt("invalid atomic transition key"));
    };
    let (kind, execution_id, ordinal) = match change {
        SessionChange::Opened { .. } => (FactKind::SessionOpen, None, 0),
        SessionChange::InputAccepted(record) => (
            FactKind::InputAccepted,
            Some(record.request.execution_id.clone()),
            0,
        ),
        SessionChange::QueueDecision(_) => (
            FactKind::QueueDecision,
            None,
            prior
                .ok_or_else(|| corrupt("queue decision precedes session open"))?
                .queue_history
                .len() as u64,
        ),
        SessionChange::SchedulingTransition { execution_id, .. } => (
            FactKind::SchedulingTransition,
            Some(execution_id.clone()),
            invocation(prior, execution_id)?.scheduling.len() as u64,
        ),
        SessionChange::ProviderObservation(event) => (
            FactKind::ProviderObservation,
            Some(event.execution_id().clone()),
            invocation(prior, event.execution_id())?.events.len() as u64,
        ),
        SessionChange::ReceiptUpdated { execution_id, .. } => (
            FactKind::ReceiptUpdated,
            Some(execution_id.clone()),
            previous_offset,
        ),
        SessionChange::StopDecision { execution_id, .. } => {
            (FactKind::StopDecision, Some(execution_id.clone()), 0)
        }
        SessionChange::ProviderReport { execution_id, .. } => {
            (FactKind::ProviderReport, Some(execution_id.clone()), 0)
        }
        SessionChange::LocalSettlement { execution_id, .. } => (
            FactKind::LocalSettlement,
            Some(execution_id.clone()),
            previous_offset,
        ),
        SessionChange::ProviderContext { .. } => (FactKind::ProviderContext, None, previous_offset),
    };
    FactKey::new(kind, execution_id, ordinal).ok_or_else(|| corrupt("invalid semantic fact key"))
}

use super::{SessionChange, SessionSnapshot, StorageError};
use crate::application::agent_execution::executions::ExecutionUpdate;
use crate::domain::agent_execution::{
    executions::{InvocationHistory, InvocationObservation, InvocationStage, QueueMutation},
    sessions::{ProviderContext, SessionId},
};
use std::collections::HashMap;

fn corrupt(message: impl std::fmt::Display) -> StorageError {
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
        None => super::validation::invocation_history(record)?,
    };
    let result = transition(&mut next)?;
    histories.insert(id.clone(), next);
    Ok(result)
}

#[derive(Clone, Copy, Default)]
struct ProviderEvidence {
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

fn invocation<'a>(
    snapshot: Option<&'a SessionSnapshot>,
    id: &ExecutionId,
) -> Result<&'a super::InvocationRecord, StorageError> {
    snapshot
        .ok_or_else(|| corrupt("fact precedes session open"))?
        .invocations
        .iter()
        .find(|record| &record.request.execution_id == id)
        .ok_or_else(|| corrupt("semantic fact has no accepted input"))
}

/// Apply an ordered, atomic SDK decision batch to an already committed state.
/// The returned candidate is validated before a caller can publish its cursor.
/// Replay has no provider, audit or tool effects.
pub(crate) fn fold_changes(
    prior: Option<&SessionSnapshot>,
    changes: &[SessionChange],
) -> Result<SessionSnapshot, StorageError> {
    if changes.is_empty() {
        return prior
            .cloned()
            .ok_or_else(|| corrupt("empty semantic batch cannot open a session"));
    }
    let mut candidate = prior.cloned();
    let mut positions = HashMap::new();
    if let Some(snapshot) = &candidate {
        for (index, record) in snapshot.invocations.iter().enumerate() {
            if positions
                .insert(record.request.execution_id.clone(), index)
                .is_some()
            {
                return Err(corrupt("execution identity occurs in multiple invocations"));
            }
        }
    }
    let mut histories = HashMap::new();
    let mut provider_evidence = ProviderEvidence::from_snapshot(candidate.as_ref());
    let mut queue = match prior {
        Some(snapshot) => super::queue_validation::QueueReplay::from_snapshot(snapshot)?,
        None => super::queue_validation::QueueReplay::empty(),
    };
    for change in changes {
        match change {
            SessionChange::Opened {
                id,
                provider,
                context,
            } => {
                if candidate.is_some() {
                    return Err(corrupt("session was opened twice"));
                }
                candidate = Some(SessionSnapshot {
                    id: id.clone(),
                    provider: provider.clone(),
                    provider_context: context.clone(),
                    invocations: Vec::new(),
                    queue_history: Vec::new(),
                });
            }
            SessionChange::InputAccepted(record) => {
                super::validation::validate_submission_acknowledgement(&record.acknowledgement)?;
                let history = super::validation::invocation_history(record)?;
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("input precedes session open"))?;
                if positions.contains_key(&record.request.execution_id) {
                    return Err(corrupt("execution identity was accepted twice"));
                }
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
                let mut next_evidence = provider_evidence;
                next_evidence.correlation |= record.target_event_offset.is_some()
                    || record.scheduling.iter().any(|event| event.target.is_some());
                next_evidence.dispatched |= record.scheduling.iter().any(|event| {
                    matches!(
                        event.stage,
                        InvocationStage::Running | InvocationStage::Injected
                    )
                });
                next_evidence.validate(&snapshot.provider_context)?;
                provider_evidence = next_evidence;
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
                let mut next_evidence = provider_evidence;
                next_evidence.selected |=
                    matches!(decision.mutation, QueueMutation::Selected { .. });
                next_evidence.validate(&snapshot.provider_context)?;
                queue.apply_current(snapshot, &positions, decision)?;
                provider_evidence = next_evidence;
                snapshot.queue_history.push(decision.clone());
            }
            SessionChange::SchedulingTransition {
                execution_id,
                event,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("scheduling precedes session open"))?;
                let mut next_evidence = provider_evidence;
                next_evidence.dispatched |= matches!(
                    event.stage,
                    InvocationStage::Running | InvocationStage::Injected
                );
                next_evidence.correlation |= event.target.is_some();
                next_evidence.validate(&snapshot.provider_context)?;
                if event.stage == InvocationStage::Running && !queue.selected(execution_id) {
                    return Err(corrupt("queued dispatch has no prior queue selection"));
                }
                let record = invocation_at(snapshot, &positions, execution_id)?;
                apply_history(&mut histories, record, |history| {
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
                provider_evidence = next_evidence;
                record.scheduling.push(event.clone());
            }
            SessionChange::ProviderObservation(event) => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("observation precedes session open"))?;
                let mut next_evidence = provider_evidence;
                next_evidence.observations = true;
                next_evidence.validate(&snapshot.provider_context)?;
                let record = invocation_at(snapshot, &positions, event.execution_id())?;
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
                apply_history(&mut histories, record, |history| {
                    history
                        .observe(event.execution_id(), observation)
                        .map_err(|error| corrupt(error.to_string()))
                })?;
                provider_evidence = next_evidence;
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
                let record = invocation_at(snapshot, &positions, execution_id)?;
                if &record.acknowledgement != before || before == after {
                    return Err(corrupt("receipt revision does not match prior value"));
                }
                super::validation::validate_submission_acknowledgement(after)?;
                record.acknowledgement = after.clone();
            }
            SessionChange::StopDecision {
                execution_id,
                event,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("stop precedes session open"))?;
                let record = invocation_at(snapshot, &positions, execution_id)?;
                if record.cancellation.is_some() {
                    return Err(corrupt("undispatched stop was already recorded"));
                }
                apply_history(&mut histories, record, |history| {
                    history
                        .record_cancellation(event.cancellation()?)
                        .map_err(|error| corrupt(error.to_string()))
                })?;
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
                let mut next_evidence = provider_evidence;
                next_evidence.reports = true;
                next_evidence.validate(&snapshot.provider_context)?;
                let record = invocation_at(snapshot, &positions, execution_id)?;
                if record.provider_report.is_some() || record.local_cancellation.is_some() {
                    return Err(corrupt("provider report was already recorded"));
                }
                if let Some(result) = &record.result {
                    super::validation::validate_report_against_local_result(report, result)?;
                }
                apply_history(&mut histories, record, |history| {
                    super::validation::record_report(
                        history,
                        Some(report),
                        local_stop.as_ref(),
                        record.scheduling.last(),
                    )
                })?;
                provider_evidence = next_evidence;
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
                let record = invocation_at(snapshot, &positions, execution_id)?;
                if &record.result != before || record.result.as_ref() == Some(after) {
                    return Err(corrupt("local result revision does not match prior value"));
                }
                super::validation::validate_local_result(record, after)?;
                let retained = apply_history(&mut histories, record, |history| {
                    history
                        .record_local_result(after.as_ref().copied().map_err(|_| ()))
                        .map_err(|error| corrupt(error.to_string()))?;
                    Ok(history.local_outcome())
                })?;
                if retained != *local_outcome {
                    return Err(corrupt("local result changes its retained outcome"));
                }
                record.local_outcome = *local_outcome;
                record.result = Some(after.clone());
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
                snapshot.provider_context = after.clone();
            }
        }
    }
    let candidate = candidate.ok_or_else(|| corrupt("semantic batch has no session"))?;
    super::validation::validate(&candidate)?;
    super::queue_validation::replay(&candidate)?;
    Ok(candidate)
}

/// A writer checks the candidate against the SDK's observed state before append.
pub(crate) fn confirm_candidate(
    session: &SessionId,
    prior: Option<&SessionSnapshot>,
    changes: &[SessionChange],
    observed: &SessionSnapshot,
) -> Result<(), StorageError> {
    if &observed.id != session {
        return Err(StorageError::IdentityMismatch);
    }
    if fold_changes(prior, changes)? != *observed {
        return Err(corrupt("semantic decisions disagree with observed session"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::agent_execution::providers::ProviderIdentity,
        domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext},
    };

    #[test]
    fn fact_identity_is_stable_across_an_uncertain_append() {
        let opened = SessionChange::Opened {
            id: SessionId::new("conversation").unwrap(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let first = key_for_changes(None, std::slice::from_ref(&opened), 0).unwrap();
        assert_eq!(first.kind(), FactKind::SessionOpen);
        assert_eq!(first.ordinal(), 0);
        assert_eq!(key_for_changes(None, &[opened], 0).unwrap(), first);

        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let pending = key_for_changes(None, std::slice::from_ref(&context), 1).unwrap();
        assert_eq!(pending.ordinal(), 1);
        assert_eq!(
            key_for_changes(None, std::slice::from_ref(&context), 1).unwrap(),
            pending
        );
        assert_ne!(
            key_for_changes(None, std::slice::from_ref(&context), 2).unwrap(),
            pending
        );
        let grouped = key_for_changes(None, &[context.clone(), context], 1).unwrap();
        assert_eq!(grouped.kind(), FactKind::AtomicTransition);
        assert_eq!(grouped.ordinal(), 1);
    }
}
