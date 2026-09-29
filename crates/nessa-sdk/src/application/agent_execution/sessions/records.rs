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
use crate::domain::agent_execution::sessions::SessionId;

fn corrupt(message: &'static str) -> StorageError {
    StorageError::Corrupt(message.into())
}

fn invocation_mut<'a>(
    snapshot: &'a mut SessionSnapshot,
    id: &ExecutionId,
) -> Result<&'a mut super::InvocationRecord, StorageError> {
    snapshot
        .invocations
        .iter_mut()
        .find(|record| &record.request.execution_id == id)
        .ok_or_else(|| corrupt("semantic fact has no accepted input"))
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
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("input precedes session open"))?;
                if snapshot
                    .invocations
                    .iter()
                    .any(|prior| prior.request.execution_id == record.request.execution_id)
                {
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
                snapshot.invocations.push((**record).clone());
            }
            SessionChange::QueueDecision(decision) => {
                candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("queue decision precedes session open"))?
                    .queue_history
                    .push(decision.clone());
            }
            SessionChange::SchedulingTransition {
                execution_id,
                event,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("scheduling precedes session open"))?;
                invocation_mut(snapshot, execution_id)?
                    .scheduling
                    .push(event.clone());
            }
            SessionChange::ProviderObservation(event) => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("observation precedes session open"))?;
                invocation_mut(snapshot, event.execution_id())?
                    .events
                    .push(event.clone());
            }
            SessionChange::ReceiptUpdated {
                execution_id,
                before,
                after,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("receipt precedes session open"))?;
                let record = invocation_mut(snapshot, execution_id)?;
                if &record.acknowledgement != before || before == after {
                    return Err(corrupt("receipt revision does not match prior value"));
                }
                record.acknowledgement = after.clone();
            }
            SessionChange::StopDecision {
                execution_id,
                event,
            } => {
                let snapshot = candidate
                    .as_mut()
                    .ok_or_else(|| corrupt("stop precedes session open"))?;
                let record = invocation_mut(snapshot, execution_id)?;
                if record.cancellation.is_some() {
                    return Err(corrupt("undispatched stop was already recorded"));
                }
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
                let record = invocation_mut(snapshot, execution_id)?;
                if record.provider_report.is_some() || record.local_cancellation.is_some() {
                    return Err(corrupt("provider report was already recorded"));
                }
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
                let record = invocation_mut(snapshot, execution_id)?;
                if &record.result != before || record.result.as_ref() == Some(after) {
                    return Err(corrupt("local result revision does not match prior value"));
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
