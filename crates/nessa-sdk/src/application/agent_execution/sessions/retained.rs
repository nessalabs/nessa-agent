//! Conservatively account owned continuation allocations, using payload owners' byte accounting.
use super::{
    InvocationCancellationEvent, InvocationRecord, InvocationSchedulingEvent, QueueHistoryRecord,
    SessionChange, SessionSnapshot, StorageError, SubmissionAcknowledgement,
};
use crate::{
    application::agent_execution::{executions::ExecutionEvent, permissions::ActionContext},
    domain::agent_execution::executions::{ExecutionId, QueueMutation},
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
fn stop(value: &InvocationCancellationEvent) -> usize {
    value.actor.as_ref().map_or(0, actor)
}

pub(super) fn snapshot(value: &SessionSnapshot) -> usize {
    #[cfg(test)]
    SNAPSHOT_ACCOUNTING_CALLS.with(|counter| counter.set(counter.get() + 1));
    let mut bytes = size_of::<SessionSnapshot>()
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
        .saturating_add(
            value
                .invocations
                .capacity()
                .saturating_mul(size_of::<InvocationRecord>()),
        )
        .saturating_add(
            value
                .queue_history
                .capacity()
                .saturating_mul(size_of::<QueueHistoryRecord>()),
        );
    for record in &value.invocations {
        bytes = bytes.saturating_add(invocation(record));
    }
    for entry in &value.queue_history {
        bytes = bytes.saturating_add(queue_entry(entry));
    }
    bytes
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
                .map(|edge| {
                    edge.actor
                        .as_ref()
                        .map_or(0, actor)
                        .saturating_add(edge.target.as_ref().map_or(0, |id| id.as_str().len()))
                })
                .fold(0usize, usize::saturating_add),
        );
    let acknowledgment = match &record.acknowledgement {
        SubmissionAcknowledgement::Pending | SubmissionAcknowledgement::Acknowledged => 0,
        SubmissionAcknowledgement::Failed { audit, storage } => audit
            .as_ref()
            .map_or(0, |error| error.retained_size().unwrap_or(usize::MAX))
            .saturating_add(storage.as_ref().map_or(0, StorageError::allocation_bytes)),
    };
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
        .saturating_add(
            record
                .result
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .map_or(0, |error| error.retained_size().unwrap_or(usize::MAX)),
        );
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

fn acknowledgement(value: &SubmissionAcknowledgement) -> usize {
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
    let base = snapshot
        .invocations
        .capacity()
        .saturating_mul(size_of::<InvocationRecord>())
        .saturating_add(
            snapshot
                .queue_history
                .capacity()
                .saturating_mul(size_of::<QueueHistoryRecord>()),
        );
    let id = super::records::affected_execution(change);
    let record = id
        .and_then(|id| positions.get(id))
        .and_then(|index| snapshot.invocations.get(*index));
    base.saturating_add(match change {
        SessionChange::Opened { .. } => size_of::<SessionSnapshot>()
            .saturating_add(snapshot.id.as_str().len())
            .saturating_add(snapshot.provider.name().len())
            .saturating_add(snapshot.provider.model_id().len())
            .saturating_add(snapshot.provider.context().len())
            .saturating_add(
                snapshot
                    .provider_context
                    .recorded()
                    .map_or(0, |id| id.as_str().len()),
            ),
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
        SessionChange::LocalSettlement { .. } => record
            .and_then(|record| record.result.as_ref())
            .and_then(|result| result.as_ref().err())
            .map_or(0, |error| error.retained_size().unwrap_or(usize::MAX)),
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
        SessionChange::SchedulingTransition { event, .. } => event
            .actor
            .as_ref()
            .map_or(0, actor)
            .saturating_add(event.target.as_ref().map_or(0, |id| id.as_str().len())),
        SessionChange::ProviderObservation(event) => event
            .retained_bytes()
            .saturating_sub(size_of::<ExecutionEvent>()),
        _ => 0,
    }
}
pub(super) fn spare_slots(
    snapshot: Option<&SessionSnapshot>,
    change: &SessionChange,
    positions: &HashMap<ExecutionId, usize>,
) -> usize {
    let Some(snapshot) = snapshot else {
        return 0;
    };
    let mut bytes = snapshot
        .invocations
        .capacity()
        .saturating_mul(size_of::<InvocationRecord>())
        .saturating_add(
            snapshot
                .queue_history
                .capacity()
                .saturating_mul(size_of::<QueueHistoryRecord>()),
        );
    // A popped new invocation drops its inner buffers. Other appends leave spare
    // slots in the same owning vector on rollback, and those slots remain charged.
    if let Some(record) = super::records::affected_execution(change)
        .and_then(|id| positions.get(id))
        .and_then(|index| snapshot.invocations.get(*index))
    {
        bytes = bytes.saturating_add(match change {
            SessionChange::SchedulingTransition { .. } => record
                .scheduling
                .capacity()
                .saturating_mul(size_of::<InvocationSchedulingEvent>()),
            SessionChange::ProviderObservation(_) => record
                .events
                .capacity()
                .saturating_mul(size_of::<ExecutionEvent>()),
            _ => 0,
        });
    }
    bytes
}
