//! Conservatively account owned continuation allocations, using payload owners' byte accounting.
use super::{
    InvocationCancellationEvent, InvocationRecord, InvocationSchedulingEvent, QueueHistoryRecord,
    SessionChange, SessionSnapshot, StorageError, SubmissionAcknowledgement,
};
use crate::{
    application::agent_execution::{
        agents::AgentError, executions::ExecutionEvent, permissions::ActionContext,
    },
    domain::agent_execution::executions::{ExecutionId, ExecutionOutcome, QueueMutation},
};
use std::{collections::HashMap, mem::size_of};

#[cfg(test)]
use std::cell::Cell;
#[cfg(test)]
std::thread_local! {
    pub(crate) static SNAPSHOT_ACCOUNTING_CALLS: Cell<usize> = const { Cell::new(0) };
}
fn actor(value: &ActionContext) -> usize {
    value
        .principal_id()
        .len()
        .saturating_add(value.surface_id().len())
        .saturating_add(value.request_id().len())
}
pub(super) fn stop(value: &InvocationCancellationEvent) -> usize {
    value.actor.as_ref().map_or(0, actor)
}

pub(super) fn snapshot(value: &SessionSnapshot) -> usize {
    #[cfg(test)]
    SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.set(counter.get() + 1));
    let mut bytes = snapshot_global(Some(value));
    for record in &value.invocations {
        bytes = bytes.saturating_add(invocation(record));
    }
    for entry in &value.queue_history {
        bytes = bytes.saturating_add(queue_entry(entry));
    }
    bytes
}

// Header and outer slots have one lifetime, separate from element payloads.
pub(super) fn snapshot_global(value: Option<&SessionSnapshot>) -> usize {
    let Some(value) = value else {
        return 0;
    };
    snapshot_header(value).saturating_add(snapshot_slots(value))
}

fn snapshot_header(value: &SessionSnapshot) -> usize {
    size_of::<SessionSnapshot>()
        .saturating_add(value.id.as_str().len())
        .saturating_add(value.provider.name().len())
        .saturating_add(value.provider.model_id().len())
        .saturating_add(value.provider.context().len())
        .saturating_add(
            value
                .provider_context
                .recorded()
                .map_or(0, |id| id.as_str().len()),
        )
}
fn snapshot_slots(value: &SessionSnapshot) -> usize {
    value
        .invocations
        .capacity()
        .saturating_mul(size_of::<InvocationRecord>())
        .saturating_add(
            value
                .queue_history
                .capacity()
                .saturating_mul(size_of::<QueueHistoryRecord>()),
        )
}

pub(super) fn invocation(record: &InvocationRecord) -> usize {
    let message = &record.request.user_message;
    let prompt = message
        .text_str()
        .len()
        .saturating_add(std::mem::size_of_val(message.images()))
        .saturating_add(std::mem::size_of_val(message.files()))
        .saturating_add(
            message
                .files()
                .iter()
                .map(|file| file.path().len())
                .fold(0usize, usize::saturating_add),
        );
    let events = record
        .events
        .iter()
        .map(ExecutionEvent::retained_bytes)
        .fold(0usize, usize::saturating_add)
        .saturating_add(
            record
                .events
                .capacity()
                .saturating_sub(record.events.len())
                .saturating_mul(size_of::<ExecutionEvent>()),
        );
    let scheduling = record
        .scheduling
        .capacity()
        .saturating_mul(size_of::<InvocationSchedulingEvent>())
        .saturating_add(
            record
                .scheduling
                .iter()
                .map(scheduling_payload)
                .fold(0usize, usize::saturating_add),
        );
    let acknowledgment = acknowledgement(&record.acknowledgement);
    let bytes = 0usize
        .saturating_add(prompt)
        .saturating_add(events)
        .saturating_add(scheduling)
        .saturating_add(record.request.execution_id.as_str().len())
        .saturating_add(actor(&record.actor))
        .saturating_add(record.cancellation.as_ref().map_or(0, stop))
        .saturating_add(record.local_cancellation.as_ref().map_or(0, stop))
        .saturating_add(acknowledgment)
        .saturating_add(
            record
                .provider_report
                .as_ref()
                .map_or(0, |report| report.retained_bytes()),
        )
        .saturating_add(result_payload(record.result.as_ref()));
    bytes
}

pub(super) fn queue_entry(entry: &QueueHistoryRecord) -> usize {
    let mutation = match &entry.mutation {
        QueueMutation::Reordered(order) => std::mem::size_of_val(order.before())
            .saturating_add(std::mem::size_of_val(order.after()))
            .saturating_add(
                order
                    .before()
                    .iter()
                    .map(|(id, _)| id.as_str().len())
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(
                order
                    .after()
                    .iter()
                    .map(|id| id.as_str().len())
                    .fold(0usize, usize::saturating_add),
            ),
        _ => entry.mutation.id().map_or(0, |id| id.as_str().len()),
    };
    let bytes = 0usize
        .saturating_add(mutation)
        .saturating_add(entry.actor.as_ref().map_or(0, actor));
    bytes
}

pub(super) fn acknowledgement(value: &SubmissionAcknowledgement) -> usize {
    match value {
        SubmissionAcknowledgement::Pending | SubmissionAcknowledgement::Acknowledged => 0,
        SubmissionAcknowledgement::Failed { audit, storage } => audit
            .as_ref()
            .map_or(0, |error| error.retained_size().unwrap_or(usize::MAX))
            .saturating_add(storage.as_ref().map_or(0, StorageError::allocation_bytes)),
    }
}
// Only slots and the component touched by this suffix are observed here.
// Existing events, messages, queue history and unrelated invocations are not read.
pub(super) fn touched(
    snapshot: Option<&SessionSnapshot>,
    change: &SessionChange,
    positions: &HashMap<ExecutionId, usize>,
    include_input: bool,
) -> usize {
    let Some(snapshot) = snapshot else {
        return 0;
    };
    let base = snapshot_slots(snapshot);
    let id = super::records::affected_execution(change);
    let record = id
        .and_then(|id| positions.get(id))
        .and_then(|index| snapshot.invocations.get(*index));
    base.saturating_add(match change {
        SessionChange::Opened { .. } => snapshot_header(snapshot),
        SessionChange::InputAccepted(_) => {
            if include_input {
                record.map_or(0, invocation)
            } else {
                0
            }
        }
        SessionChange::SchedulingTransition { .. } => record.map_or(0, |record| {
            record
                .scheduling
                .capacity()
                .saturating_mul(size_of::<InvocationSchedulingEvent>())
        }),
        SessionChange::ProviderObservation(_) => record.map_or(0, |record| {
            record
                .events
                .capacity()
                .saturating_mul(size_of::<ExecutionEvent>())
        }),
        SessionChange::ReceiptUpdated { .. } => {
            record.map_or(0, |record| acknowledgement(&record.acknowledgement))
        }
        SessionChange::StopDecision { .. } => record
            .and_then(|record| record.cancellation.as_ref())
            .map_or(0, stop),
        SessionChange::ProviderReport { .. } => record.map_or(0, |record| {
            record
                .provider_report
                .as_ref()
                .map_or(0, |report| report.retained_bytes())
                .saturating_add(record.local_cancellation.as_ref().map_or(0, stop))
        }),
        SessionChange::LocalSettlement { .. } => {
            result_payload(record.and_then(|record| record.result.as_ref()))
        }
        SessionChange::ProviderContext { .. } => snapshot
            .provider_context
            .recorded()
            .map_or(0, |id| id.as_str().len()),
        SessionChange::QueueDecision(_) => 0,
    })
}
pub(super) fn append_payload(change: &SessionChange) -> usize {
    match change {
        SessionChange::QueueDecision(entry) => queue_entry(entry),
        SessionChange::SchedulingTransition { event, .. } => scheduling_payload(event),
        SessionChange::ProviderObservation(event) => observation_payload(event),
        _ => 0,
    }
}
pub(super) fn scheduling_payload(event: &InvocationSchedulingEvent) -> usize {
    event
        .actor
        .as_ref()
        .map_or(0, actor)
        .saturating_add(event.target.as_ref().map_or(0, |id| id.as_str().len()))
}
pub(super) fn observation_payload(event: &ExecutionEvent) -> usize {
    event
        .retained_bytes()
        .saturating_sub(size_of::<ExecutionEvent>())
}
pub(super) fn result_payload(result: Option<&Result<ExecutionOutcome, AgentError>>) -> usize {
    result
        .and_then(|result| result.as_ref().err())
        .map_or(0, |error| error.retained_size().unwrap_or(usize::MAX))
}
