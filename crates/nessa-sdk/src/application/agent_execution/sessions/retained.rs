//! Conservatively account owned continuation allocations, using payload owners' byte accounting.
use super::{
    InvocationCancellationEvent, InvocationRecord, InvocationSchedulingEvent, QueueHistoryRecord,
    SessionSnapshot, StorageError, SubmissionAcknowledgement,
};
use crate::{
    application::agent_execution::{executions::ExecutionEvent, permissions::ActionContext},
    domain::agent_execution::executions::QueueMutation,
};
use std::mem::size_of;

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
        bytes = bytes
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
    }
    for entry in &value.queue_history {
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
        bytes = bytes
            .saturating_add(mutation)
            .saturating_add(entry.actor.as_ref().map_or(0, actor));
    }
    bytes
}
