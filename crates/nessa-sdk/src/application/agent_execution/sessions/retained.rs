//! Conservatively account owned continuation allocations, using payload owners' byte accounting.
use super::{
    ArtifactRecord, CurrentLease, CurrentLeaseState, FetchOutcome, HandoffKind, HandoffRecord,
    InvocationCancellationEvent, InvocationRecord, InvocationSchedulingEvent, LeaseRecord, NoteId,
    NoteRecord, QueueHistoryRecord, SessionChange, SessionSnapshot, StorageError,
    SubmissionAcknowledgement,
};
use crate::{
    application::agent_execution::{
        agents::AgentError, executions::ExecutionEvent, permissions::ActionContext,
    },
    domain::agent_execution::{
        executions::{ExecutionId, ExecutionOutcome, Initiation, Initiator, QueueMutation},
        leases::{CommandTerms, EnvironmentRef, LeaseTerms, LeaseWork, PeerEnvironment},
        prompts::{AppModelContext, MessageSender},
    },
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
/// The grant's identity text; an owner holds no text of its own.
fn initiator(value: &Initiator) -> usize {
    match value {
        Initiator::Owner => 0,
        Initiator::Grant(grant) => grant.grant().len(),
    }
}
fn initiation(value: &Initiation) -> usize {
    initiator(value.initiator())
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
    for record in &value.artifacts {
        bytes = bytes.saturating_add(artifact(record));
    }
    for record in &value.notes {
        bytes = bytes.saturating_add(note(record));
    }
    for record in &value.handoffs {
        bytes = bytes.saturating_add(handoff(record));
    }
    bytes.saturating_add(lease(value.lease.as_ref()))
}

/// One note's payload beyond its slot in the list.
pub(super) fn note(record: &NoteRecord) -> usize {
    record
        .id
        .as_str()
        .len()
        .saturating_add(record.text.as_str().len())
        .saturating_add(actor(&record.actor))
        .saturating_add(initiator(&record.author))
        .saturating_add(
            record
                .after_execution
                .as_ref()
                .map_or(0, |turn| turn.as_str().len()),
        )
}

/// One handoff's payload beyond its slot in the list: its identities, a
/// switch's two provider identities, and a fork's copied turns.
pub(super) fn handoff(record: &HandoffRecord) -> usize {
    let kind = match &record.kind {
        HandoffKind::Switch { before, after } => [before, after]
            .into_iter()
            .map(|identity| {
                identity
                    .name()
                    .len()
                    .saturating_add(identity.model_id().len())
                    .saturating_add(identity.context().len())
            })
            .fold(0usize, usize::saturating_add),
        HandoffKind::Fork { source, history } => source
            .as_str()
            .len()
            .saturating_add(std::mem::size_of_val(history.turns()))
            .saturating_add(history.text_bytes()),
    };
    record
        .id
        .as_str()
        .len()
        .saturating_add(actor(&record.actor))
        .saturating_add(
            record
                .after_execution
                .as_ref()
                .map_or(0, |turn| turn.as_str().len()),
        )
        .saturating_add(kind)
}

/// One artifact record's payload beyond its slot in the list.
pub(super) fn artifact(record: &ArtifactRecord) -> usize {
    record
        .lease
        .as_str()
        .len()
        .saturating_add(record.turn.as_ref().map_or(0, |turn| turn.as_str().len()))
        .saturating_add(record.name.as_str().len())
        .saturating_add(record.file.media_type().as_str().len())
        .saturating_add(actor(&record.actor))
}

/// The latest lease and its records, which a lease change replaces whole.
pub(super) fn lease(value: Option<&CurrentLease>) -> usize {
    let Some(value) = value else {
        return 0;
    };
    let records = value
        .records()
        .iter()
        .map(lease_record)
        .fold(0usize, usize::saturating_add);
    size_of::<CurrentLease>()
        .saturating_add(records)
        .saturating_add(match value.state() {
            CurrentLeaseState::Held(lease) => lease_terms(lease.terms()),
            CurrentLeaseState::Refused { lease, .. } => lease.as_str().len(),
            CurrentLeaseState::Unreadable { kind } => kind.len(),
        })
}

fn lease_terms(terms: &LeaseTerms) -> usize {
    match &terms.work {
        LeaseWork::Agent(work) => work.agent().len().saturating_add(work.model().len()),
    }
}

fn lease_record(record: &LeaseRecord) -> usize {
    size_of::<LeaseRecord>().saturating_add(match record {
        LeaseRecord::Issued {
            lease,
            terms,
            actor: who,
            ..
        }
        | LeaseRecord::Refused {
            lease,
            terms,
            actor: who,
            ..
        } => lease
            .as_str()
            .len()
            .saturating_add(lease_terms(terms))
            .saturating_add(actor(who)),
        LeaseRecord::Ending {
            lease, actor: who, ..
        } => lease
            .as_str()
            .len()
            .saturating_add(who.as_ref().map_or(0, actor)),
        LeaseRecord::Ended { lease, .. }
        | LeaseRecord::Interrupted { lease }
        | LeaseRecord::CleanupReported { lease, .. } => lease.as_str().len(),
        LeaseRecord::EventDropped { lease, turn, .. } => {
            lease.as_str().len().saturating_add(turn.as_str().len())
        }
        LeaseRecord::CommandIssued {
            lease,
            parent,
            terms,
            actor: who,
        }
        | LeaseRecord::CommandRefused {
            lease,
            parent,
            terms,
            actor: who,
            ..
        } => lease
            .as_str()
            .len()
            .saturating_add(parent.as_str().len())
            .saturating_add(command_terms(terms))
            .saturating_add(actor(who)),
        LeaseRecord::CommandEnded {
            lease,
            parent,
            output,
            ..
        } => lease
            .as_str()
            .len()
            .saturating_add(parent.as_str().len())
            .saturating_add(output.stdout_tail().len())
            .saturating_add(output.stderr_tail().len()),
        LeaseRecord::FetchAsked {
            lease,
            request,
            file,
            destination,
            actor: who,
        } => lease
            .as_str()
            .len()
            .saturating_add(request.len())
            .saturating_add(file.media_type().as_str().len())
            .saturating_add(destination.as_str().len())
            .saturating_add(actor(who)),
        LeaseRecord::FetchEnded {
            lease,
            request,
            outcome,
        } => lease
            .as_str()
            .len()
            .saturating_add(request.len())
            .saturating_add(match outcome {
                FetchOutcome::Placed { path } | FetchOutcome::AlreadyPlaced { path } => {
                    path.as_str().len()
                }
                FetchOutcome::Refused(_) => 0,
            }),
        LeaseRecord::Unreadable { kind, body } => kind.len().saturating_add(body.len()),
    })
}

fn command_terms(terms: &CommandTerms) -> usize {
    let environment = match &terms.environment {
        EnvironmentRef::Here => 0,
        EnvironmentRef::Ssh(host) => host.as_str().len(),
        EnvironmentRef::Boat => "boat:thread".len(),
        EnvironmentRef::Peer(_) => PeerEnvironment::PREFIX.len() + 2 * PeerEnvironment::KEY_BYTES,
    };
    terms
        .command
        .argv()
        .iter()
        .map(|argument| argument.len())
        .fold(environment, usize::saturating_add)
        .saturating_add(terms.command.cwd().map_or(0, str::len))
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
                .model_change
                .as_ref()
                .map_or(0, |record| record.retained_payload_bytes()),
        )
        .saturating_add(
            value
                .current_configuration
                .as_ref()
                .map_or(0, |record| record.retained_payload_bytes()),
        )
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
        .saturating_add(
            value
                .artifacts
                .capacity()
                .saturating_mul(size_of::<ArtifactRecord>()),
        )
        .saturating_add(
            value
                .notes
                .capacity()
                .saturating_mul(size_of::<NoteRecord>()),
        )
        .saturating_add(
            value
                .handoffs
                .capacity()
                .saturating_mul(size_of::<HandoffRecord>()),
        )
}

pub(super) fn invocation(record: &InvocationRecord) -> usize {
    let message = &record.request.user_message;
    let prompt = message
        .text_str()
        .len()
        .saturating_add(std::mem::size_of_val(message.images()))
        .saturating_add(std::mem::size_of_val(message.files()))
        .saturating_add(std::mem::size_of_val(message.uploads()))
        .saturating_add(
            message
                .uploads()
                .iter()
                .map(|file| file.name().len() + file.media_type().as_str().len())
                .sum::<usize>(),
        )
        .saturating_add(
            message
                .files()
                .iter()
                .map(|file| file.path().len())
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(match message.sender() {
            MessageSender::Person | MessageSender::Internal => 0,
            MessageSender::App(app) => app.payload_bytes(),
        })
        .saturating_add(std::mem::size_of_val(message.app_model_context()))
        .saturating_add(
            message
                .app_model_context()
                .iter()
                .map(AppModelContext::payload_bytes)
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
        .saturating_add(initiation(&record.initiation))
        .saturating_add(
            record
                .notes
                .capacity()
                .saturating_mul(size_of::<NoteId>())
                .saturating_add(
                    record
                        .notes
                        .iter()
                        .map(|note| note.as_str().len())
                        .fold(0usize, usize::saturating_add),
                ),
        )
        .saturating_add(record.handoff.as_ref().map_or(0, |id| id.as_str().len()))
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
        SessionChange::Opened { .. }
        | SessionChange::ModelChanged(_)
        | SessionChange::ConfigurationChanged(_)
        | SessionChange::ProviderIdentityChanged { .. }
        | SessionChange::Handoff(_) => snapshot_header(snapshot),
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
        SessionChange::Lease(_) => lease(snapshot.lease.as_ref()),
        SessionChange::Artifact(_) | SessionChange::Note(_) | SessionChange::NoteRetired(_) => 0,
    })
}
pub(super) fn append_payload(change: &SessionChange) -> usize {
    match change {
        SessionChange::QueueDecision(entry) => queue_entry(entry),
        SessionChange::SchedulingTransition { event, .. } => scheduling_payload(event),
        SessionChange::ProviderObservation(event) => observation_payload(event),
        SessionChange::Artifact(record) => artifact(record),
        SessionChange::Note(record) => note(record),
        SessionChange::Handoff(record) => handoff(record),
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
