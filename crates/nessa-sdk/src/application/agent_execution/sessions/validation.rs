//! Validate snapshot relationships at every storage port, including custom adapters.
mod observations;
use super::app_sources;
use super::retention::{Witness, WitnessUndo};
use super::steering_position::SteeringPosition;
use super::{
    InvocationCancellationEvent, InvocationRecord, InvocationSchedulingEvent, ProviderContext,
    SessionSnapshot, StorageError, SubmissionAcknowledgement,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::{
    limits::{validate_observation_id, ObservationUsage, MAX_RETAINED_OUTPUT_EVENTS},
    ExecutionEvent, ExecutionUpdate,
};
use crate::application::agent_execution::providers::{ExecutionReport, ExecutionReportSource};
use crate::domain::agent_execution::{
    executions::{
        ExecutionOutcome, InvocationHistory, InvocationObservation, InvocationStage, QueueMutation,
    },
    tools::{McpTool, ToolCallId},
};
use observations::{ObservationUndo, Observations};
use std::{collections::HashMap, fmt::Display};

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
std::thread_local! {
    pub(crate) static VALIDATION_CALLS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

fn corrupt(error: impl Display) -> StorageError {
    StorageError::Corrupt(error.to_string())
}

pub(super) struct InvocationContinuation {
    pub(super) history: Option<InvocationHistory>,
    usage: ObservationUsage,
    observations: Observations,
    witness: Option<Witness>,
}
pub(super) struct InvocationObservationUndo {
    usage: ObservationUsage,
    observation: ObservationUndo,
    witness: WitnessUndo,
    created_witness: bool,
}
impl InvocationContinuation {
    pub(super) fn rebuild(
        snapshot: &SessionSnapshot,
        invocation: &InvocationRecord,
    ) -> Result<Self, StorageError> {
        let mut state = Self::empty(invocation)?;
        for (index, event) in invocation.events.iter().enumerate() {
            state.observe(&snapshot.provider_context, invocation, index, event)?;
        }
        Ok(state)
    }
    pub(super) fn empty(invocation: &InvocationRecord) -> Result<Self, StorageError> {
        invocation
            .request
            .validate_message_size()
            .map_err(corrupt)?;
        if invocation.events.capacity() > MAX_RETAINED_OUTPUT_EVENTS {
            return Err(corrupt(
                "retained observation capacity exceeds per-invocation limit",
            ));
        }
        if let Some(first) = invocation.scheduling.first() {
            validate_admission_actor(invocation, first)?;
        }
        Ok(Self {
            history: Some(invocation_history(invocation)?),
            usage: ObservationUsage::default(),
            observations: Observations::default(),
            witness: None,
        })
    }
    pub(super) fn observe(
        &mut self,
        context: &ProviderContext,
        record: &InvocationRecord,
        index: usize,
        event: &ExecutionEvent,
    ) -> Result<InvocationObservationUndo, StorageError> {
        event.validate_payload_size().map_err(corrupt)?;
        let usage = self.usage.with_event(event).map_err(corrupt)?;
        let observation = self.observations.observe(context, record, index, event)?;
        let created_witness = self.witness.is_none();
        if created_witness {
            let witness = context
                .recorded()
                .ok_or_else(|| corrupt("provider observations require context"))
                .and_then(|context| {
                    Witness::new(context.clone(), record.request.execution_id.clone())
                        .map_err(corrupt)
                });
            match witness {
                Ok(witness) => self.witness = Some(witness),
                Err(error) => {
                    self.observations.restore(observation);
                    return Err(error);
                }
            }
        }
        let witness = match self
            .witness
            .as_mut()
            .expect("initialized witness")
            .observe(event)
        {
            Ok(undo) => undo,
            Err(error) => {
                self.observations.restore(observation);
                if created_witness {
                    self.witness = None;
                }
                return Err(corrupt(error));
            }
        };
        let old_usage = std::mem::replace(&mut self.usage, usage);
        Ok(InvocationObservationUndo {
            usage: old_usage,
            observation,
            witness,
            created_witness,
        })
    }
    pub(super) fn restore_observation(&mut self, undo: InvocationObservationUndo) {
        self.witness
            .as_mut()
            .expect("retained witness")
            .restore(undo.witness);
        if undo.created_witness {
            self.witness = None;
        }
        self.observations.restore(undo.observation);
        self.usage = undo.usage;
    }
    /// The MCP tool `tool_id` was observed calling in `record`, the
    /// invocation this continuation is of, with the index of the event that
    /// first observed it so.
    pub(super) fn mcp_tool<'a>(
        &self,
        record: &'a InvocationRecord,
        tool_id: &ToolCallId,
    ) -> Option<(usize, &'a McpTool)> {
        self.observations.mcp_tool_call(tool_id).and_then(|index| {
            app_sources::mcp_tool_call(record.events[index].update()).map(|(_, tool)| (index, tool))
        })
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.observations
            .retained_bytes()
            .saturating_add(self.witness.as_ref().map_or(0, Witness::retained_bytes))
    }
}

pub(crate) fn validate(snapshot: &SessionSnapshot) -> Result<(), StorageError> {
    continuation(snapshot).map(drop)
}

pub(super) fn continuation(
    snapshot: &SessionSnapshot,
) -> Result<Vec<InvocationContinuation>, StorageError> {
    #[cfg(test)]
    VALIDATION_CALLS.with(|calls| {
        let (full, history) = calls.get();
        calls.set((full + 1, history));
    });
    if snapshot.invocations.len() > SessionSnapshot::MAX_INVOCATIONS {
        return Err(corrupt("retained invocation history exceeds session limit"));
    }
    let _ = super::queue_validation::replay(snapshot)?;
    snapshot
        .provider_context
        .validate_evidence(
            snapshot
                .invocations
                .iter()
                .any(|invocation| !invocation.events.is_empty()),
            snapshot
                .invocations
                .iter()
                .any(|invocation| invocation.provider_report.is_some()),
            snapshot.invocations.iter().any(|invocation| {
                invocation.scheduling.iter().any(|event| {
                    matches!(
                        event.stage,
                        InvocationStage::Running | InvocationStage::Injected
                    )
                })
            }),
            SteeringPosition::any_saved(&snapshot.invocations)?,
            snapshot
                .queue_history
                .iter()
                .any(|record| matches!(&record.mutation, QueueMutation::Selected { .. })),
        )
        .map_err(corrupt)?;
    let mut states = Vec::with_capacity(snapshot.invocations.len());
    let mut identities = HashMap::with_capacity(snapshot.invocations.len());
    let mut event_counts = HashMap::new();
    // The invocations already walked, by identity: a message is asked
    // against those before it, through each one's observation index.
    let mut positions = HashMap::with_capacity(snapshot.invocations.len());
    for invocation in &snapshot.invocations {
        // The snapshot holds a steered message's target whole, so the offset
        // bounds which of its calls came before the message.
        let steered = SteeringPosition::saved(invocation)?;
        app_sources::validate_saved(
            &invocation.request.user_message,
            steered,
            |execution, tool| {
                positions.get(execution).and_then(|&index: &usize| {
                    InvocationContinuation::mcp_tool(
                        &states[index],
                        &snapshot.invocations[index],
                        tool,
                    )
                })
            },
        )
        .map_err(corrupt)?;
        if let Some(position) = steered {
            // A target with no preceding history at all fails the same way an
            // offset past that history does: neither can be a position in it.
            if event_counts
                .get(position.target())
                .is_none_or(|count| position.offset() > *count)
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
        let state = InvocationContinuation::rebuild(snapshot, invocation)?;
        let history = state.history.as_ref().expect("rebuilt history");
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
            validate_admission_actor(invocation, first)?;
        }
        if let Some(target) = steered.map(SteeringPosition::target) {
            if target == &invocation.request.execution_id || !identities.contains_key(target) {
                return Err(corrupt("steering target is not a prior invocation"));
            }
            if identities.get(target) != Some(&true) {
                return Err(corrupt("steering target was never dispatched"));
            }
        }
        positions.insert(&invocation.request.execution_id, states.len());
        states.push(state);
    }
    Ok(states)
}

pub(super) fn validate_admission_actor(
    record: &InvocationRecord,
    event: &InvocationSchedulingEvent,
) -> Result<(), StorageError> {
    if event.actor.as_ref() != Some(&record.actor) {
        return Err(corrupt(
            "submission actor disagrees with scheduling admission",
        ));
    }
    Ok(())
}

/// A context-bound observation must agree with the context at its own fact,
/// before a later provider-context revision can change the final projection.
pub(super) fn validate_observation_context(
    context: &ProviderContext,
    event: &ExecutionEvent,
) -> Result<(), StorageError> {
    let ExecutionUpdate::PermissionCancelled(record) = event.update() else {
        return Ok(());
    };
    validate_observation_id(record.request().id().as_str()).map_err(corrupt)?;
    validate_observation_id(record.request().tool_id().as_str()).map_err(corrupt)?;
    if context.recorded() != Some(record.session_id())
        || record.request().execution_id() != event.execution_id()
    {
        return Err(corrupt(
            "cancellation belongs to a different session or invocation",
        ));
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
    validate_submission_acknowledgement(&invocation.acknowledgement)?;
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

/// Check one acknowledgement before it can be overwritten by another fact.
pub(super) fn validate_submission_acknowledgement(
    acknowledgement: &SubmissionAcknowledgement,
) -> Result<(), StorageError> {
    if matches!(
        acknowledgement,
        SubmissionAcknowledgement::Failed {
            audit: None,
            storage: None
        }
    ) {
        return Err(corrupt(
            "failed submission acknowledgement has no failed boundary",
        ));
    }
    if let SubmissionAcknowledgement::Failed { audit, storage } = acknowledgement {
        if let Some(error) = audit {
            error.validate_retained_size()?;
        }
        if let Some(error) = storage {
            error.validate_retained_size()?;
        }
    }
    Ok(())
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
    if let Err(error) = result {
        error.validate_retained_size()?;
    }
    if let Some(settlement) = &invocation.provider_report {
        validate_report_against_local_result(settlement, result)?;
    }
    Ok(())
}

/// A newly recorded report cannot retroactively invalidate an earlier public
/// success, even if a later diagnostic in the same batch would hide that result.
pub(super) fn validate_report_against_local_result(
    settlement: &ExecutionReport,
    result: &Result<ExecutionOutcome, AgentError>,
) -> Result<(), StorageError> {
    if result.is_ok() {
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

/// Apply one proposed local result against the record's prior history authority.
/// The caller mutates its projection only after this transition is accepted.
pub(super) fn local_result_transition(
    invocation: &InvocationRecord,
    result: &Result<ExecutionOutcome, AgentError>,
) -> Result<Option<ExecutionOutcome>, StorageError> {
    validate_local_result(invocation, result)?;
    let mut history = invocation_history(invocation)?;
    history
        .record_local_result(result.as_ref().copied().map_err(|_| ()))
        .map_err(corrupt)?;
    Ok(history.local_outcome())
}
