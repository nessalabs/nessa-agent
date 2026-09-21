//! Validate snapshot relationships at every storage port, including custom adapters.
use super::{
    InvocationCancellationEvent, InvocationRecord, InvocationSchedulingEvent, SessionSnapshot,
    StorageError,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::{
    limits::{validate_observation_id, ObservationUsage, MAX_RETAINED_OUTPUT_EVENTS},
    ExecutionUpdate,
};
use crate::application::agent_execution::providers::{ExecutionReport, ExecutionReportSource};
use crate::domain::agent_execution::{
    executions::{ExecutionOutcome, InvocationHistory, InvocationObservation, InvocationStage},
    permissions::{PermissionRequest, PermissionStateView},
};
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
};

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
std::thread_local! {
    pub(super) static VALIDATION_CALLS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

fn corrupt(error: impl Display) -> StorageError {
    StorageError::Corrupt(error.to_string())
}

pub(crate) fn validate(snapshot: &SessionSnapshot) -> Result<(), StorageError> {
    #[cfg(test)]
    VALIDATION_CALLS.with(|calls| {
        let (full, history) = calls.get();
        calls.set((full + 1, history));
    });
    let _ = super::queue_validation::replay(snapshot)?;
    let mut identities = HashMap::with_capacity(snapshot.invocations.len());
    let mut event_counts = HashMap::new();
    for invocation in &snapshot.invocations {
        if let Some(offset) = invocation.target_event_offset {
            let target = invocation
                .scheduling
                .first()
                .and_then(|edge| edge.target.as_ref());
            // A target with no preceding history at all fails the same way an
            // offset past that history does: neither can be a position in it.
            if target
                .and_then(|target| event_counts.get(target))
                .is_none_or(|count| offset > *count)
            {
                return Err(corrupt(
                    "steering offset is outside the preceding target history",
                ));
            }
        }
        event_counts.insert(
            invocation.request.execution_id.clone(),
            invocation.events.len(),
        );
        if invocation.events.capacity() > MAX_RETAINED_OUTPUT_EVENTS {
            return Err(corrupt(
                "retained observation capacity exceeds per-invocation limit",
            ));
        }
        ObservationUsage::from_events(&invocation.events).map_err(corrupt)?;
        if let Some(Err(error)) = &invocation.result {
            error.validate_retained_size()?;
        }
        invocation
            .request
            .validate_message_size()
            .map_err(corrupt)?;
        let history = invocation_history(invocation)?;
        // Derive target eligibility once per record, without rescanning a long
        // prior execution for every subsequent steering input.
        if identities
            .insert(
                &invocation.request.execution_id,
                history.validate_steering_target().is_ok(),
            )
            .is_some()
        {
            return Err(corrupt("execution identity occurs in multiple invocations"));
        }
        if let Some(first) = invocation.scheduling.first() {
            if first.actor.as_ref() != Some(&invocation.actor) {
                return Err(corrupt(
                    "submission actor disagrees with scheduling admission",
                ));
            }
            if let Some(target) = &first.target {
                if target == &invocation.request.execution_id || !identities.contains_key(target) {
                    return Err(corrupt("steering target is not a prior invocation"));
                }
                if identities.get(target) != Some(&true) {
                    return Err(corrupt("steering target was never dispatched"));
                }
            }
        }
        for event in &invocation.events {
            event.validate_payload_size().map_err(corrupt)?;
        }
        super::retention::validate(snapshot, invocation).map_err(corrupt)?;
        let mut reviews = HashMap::new();
        let mut review_ids = HashSet::new();
        for event in &invocation.events {
            match event.update() {
                ExecutionUpdate::Tool(tool) => {
                    validate_observation_id(tool.id().as_str()).map_err(corrupt)?
                }
                ExecutionUpdate::PermissionRequested {
                    id,
                    tool_id,
                    options,
                    input,
                    ..
                } => {
                    validate_observation_id(id.as_str()).map_err(corrupt)?;
                    validate_observation_id(tool_id.as_str()).map_err(corrupt)?;
                    if !review_ids.insert(id) {
                        return Err(corrupt(
                            "permission identity is repeated within an invocation",
                        ));
                    }
                    reviews.insert(
                        id,
                        (
                            PermissionRequest::new(
                                id.clone(),
                                event.execution_id().clone(),
                                tool_id.clone(),
                                options.clone(),
                            ),
                            input,
                        ),
                    );
                }
                ExecutionUpdate::PermissionCancelled(record) => {
                    validate_observation_id(record.request().id().as_str()).map_err(corrupt)?;
                    validate_observation_id(record.request().tool_id().as_str())
                        .map_err(corrupt)?;
                    if record.session_id() != &snapshot.provider_session_id
                        || record.request().execution_id() != event.execution_id()
                    {
                        return Err(corrupt(
                            "cancellation belongs to a different session or invocation",
                        ));
                    }
                    let (mut pending, input) = reviews
                        .remove(record.request().id())
                        .ok_or_else(|| corrupt("cancellation has no preceding pending request"))?;
                    let PermissionStateView::Cancelled { reason } = record.request().state() else {
                        return Err(corrupt("cancellation request is not cancelled"));
                    };
                    pending.cancel(reason.clone()).map_err(corrupt)?;
                    if &pending != record.request() || input != record.input() {
                        return Err(corrupt(
                            "cancellation differs from the original permission request",
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Rebuild history authority from a boundary projection. Live mutations and
/// restoration use this same entity; the DTO cannot authorize a transition.
pub(super) fn invocation_history(
    invocation: &InvocationRecord,
) -> Result<InvocationHistory, StorageError> {
    #[cfg(test)]
    VALIDATION_CALLS.with(|calls| {
        let (full, history) = calls.get();
        calls.set((full, history + 1));
    });
    if let Some(result) = &invocation.result {
        validate_local_result(invocation, result)?;
    }
    let mut history = InvocationHistory::new(
        invocation.request.execution_id.clone(),
        invocation.submission,
    );
    if let Some(cancellation) = &invocation.cancellation {
        history
            .record_cancellation(cancellation.cancellation()?)
            .map_err(corrupt)?;
    }
    for event in &invocation.scheduling {
        history
            .schedule(event.transition().map_err(corrupt)?)
            .map_err(corrupt)?;
    }
    if let Some(outcome) = invocation.local_outcome {
        history.record_local_outcome(outcome).map_err(corrupt)?;
    }
    if let Some(result) = &invocation.result {
        history
            .record_local_result(result.as_ref().copied().map_err(|_| ()))
            .map_err(corrupt)?;
    }
    for event in &invocation.events {
        let observation = match event.update() {
            ExecutionUpdate::Finished(outcome) => InvocationObservation::Finished(*outcome),
            ExecutionUpdate::PermissionCancelled(_) => {
                InvocationObservation::PermissionCancellation
            }
            _ => InvocationObservation::Output,
        };
        history
            .observe(event.execution_id(), observation)
            .map_err(corrupt)?;
    }
    record_report(
        &mut history,
        invocation.provider_report.as_ref(),
        invocation.local_cancellation.as_ref(),
        invocation.scheduling.last(),
    )?;
    history.validate_checkpoint().map_err(corrupt)?;
    Ok(history)
}

/// Map report origin and local stop together through the domain's history authority.
pub(super) fn record_report(
    history: &mut InvocationHistory,
    report: Option<&ExecutionReport>,
    cancellation: Option<&InvocationCancellationEvent>,
    scheduling: Option<&InvocationSchedulingEvent>,
) -> Result<(), StorageError> {
    validate_stop_actor(cancellation, scheduling)?;
    match (report, cancellation) {
        (Some(report), Some(cancellation))
            if report.source() == ExecutionReportSource::LocalCancellation =>
        {
            history
                .record_local_cancellation(cancellation.cancellation()?)
                .map_err(corrupt)
        }
        (Some(report), None) if report.source() == ExecutionReportSource::Provider => history
            .record_provider_result(
                report
                    .provider_result()
                    .map(|result| result.as_ref().copied().map_err(|_| ())),
            )
            .map_err(corrupt),
        (None, None) => Ok(()),
        _ => Err(corrupt(
            "local cancellation report and invocation stop evidence must agree",
        )),
    }
}

/// Domain transitions validate cause and initiator kind; verified actor identity
/// remains an application fact and must match both views of the same local stop.
pub(super) fn validate_stop_actor(
    cancellation: Option<&InvocationCancellationEvent>,
    scheduling: Option<&InvocationSchedulingEvent>,
) -> Result<(), StorageError> {
    if let (Some(cancellation), Some(scheduling)) = (cancellation, scheduling) {
        if scheduling.stage == InvocationStage::Cancelled && cancellation.actor != scheduling.actor
        {
            return Err(corrupt(
                "scheduling cancellation names a different invocation stop actor",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_local_result(
    invocation: &InvocationRecord,
    result: &Result<ExecutionOutcome, AgentError>,
) -> Result<(), StorageError> {
    if let (Some(settlement), Ok(_)) = (&invocation.provider_report, result) {
        // The domain checks outcome agreement, including causally recorded local
        // cancellation. Public success still cannot hide report delivery or cleanup
        // failures, which the pure history deliberately does not interpret.
        if settlement.clone().into_result().is_err() {
            return Err(corrupt(
                "successful local settlement contradicts provider settlement facts",
            ));
        }
    }
    Ok(())
}
