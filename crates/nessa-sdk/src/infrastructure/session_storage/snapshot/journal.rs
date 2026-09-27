//! Each complete line replaces one snapshot logically, while storing only changed tails.
use super::{
    queue_order::QueueEvent,
    records::{Event, Metadata, Provider},
    scheduling::SchedulingEvent,
    tools::corrupt,
    validate,
};
use crate::application::agent_execution::sessions::{
    InvocationRecord, SessionSnapshot, StorageError,
};
use crate::domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext, SessionId};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    sequence: u64,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_context: Option<String>,
    queue_from: usize,
    queue_history: Vec<QueueEvent>,
    invocation_count: usize,
    invocations: Vec<InvocationChange>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvocationChange {
    index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Metadata>,
    events_from: usize,
    events: Vec<Event>,
    scheduling_from: usize,
    scheduling: Vec<SchedulingEvent>,
}

pub(in crate::infrastructure::session_storage) struct Loaded {
    pub snapshot: Option<SessionSnapshot>,
    pub sequence: u64,
    /// Byte boundary after the last complete line; trailing bytes are uncommitted.
    pub committed_bytes: u64,
    pub incomplete: bool,
}

/// Replay complete records. Never accept a malformed complete line as an empty session.
pub(in crate::infrastructure::session_storage) fn read(
    reader: impl Read + Seek,
    id: &SessionId,
) -> Result<Loaded, StorageError> {
    let mut reader = BufReader::new(reader);
    let mut snapshot = None;
    let mut sequence = 0_u64;
    let mut committed_bytes = 0_u64;
    let incomplete = loop {
        let mut count = 0_u64;
        let complete = loop {
            let buffer = reader.fill_buf().map_err(io_error)?;
            if buffer.is_empty() {
                break false;
            }
            let newline = buffer.iter().position(|byte| *byte == b'\n');
            let used = newline.map_or(buffer.len(), |index| index + 1);
            reader.consume(used);
            count = count
                .checked_add(used as u64)
                .ok_or_else(|| corrupt("journal length overflow"))?;
            if newline.is_some() {
                break true;
            }
        };
        if !complete {
            break count != 0;
        }
        // Scan before decoding: an incomplete final line never allocates its payload.
        reader
            .seek(SeekFrom::Start(committed_bytes))
            .map_err(io_error)?;
        super::decode::preflight(
            (&mut reader).take(count),
            snapshot
                .as_ref()
                .map_or(0, |value: &SessionSnapshot| value.invocations.len()),
        )?;
        reader
            .seek(SeekFrom::Start(committed_bytes))
            .map_err(io_error)?;
        let record: Record = serde_json::from_reader((&mut reader).take(count)).map_err(corrupt)?;
        if record.id != id.as_str() {
            return Err(StorageError::IdentityMismatch);
        }
        let expected = sequence
            .checked_add(1)
            .ok_or_else(|| corrupt("journal sequence exhausted"))?;
        if record.sequence != expected {
            return Err(corrupt("journal sequence is not consecutive"));
        }
        apply(&mut snapshot, record)?;
        sequence = expected;
        committed_bytes = committed_bytes
            .checked_add(count)
            .ok_or_else(|| corrupt("journal length overflow"))?;
    };
    Ok(Loaded {
        snapshot,
        sequence,
        committed_bytes,
        incomplete,
    })
}

fn apply(snapshot: &mut Option<SessionSnapshot>, record: Record) -> Result<(), StorageError> {
    if snapshot.is_none() {
        *snapshot = Some(SessionSnapshot {
            queue_history: Vec::new(),
            id: SessionId::new(record.id.clone()).map_err(corrupt)?,
            provider: record
                .provider
                .ok_or_else(|| corrupt("first journal record has no provider"))?
                .decode()?,
            provider_context: record
                .provider_context
                .map(ExecutionSessionId::new)
                .transpose()
                .map_err(corrupt)?
                .map_or(ProviderContext::Absent, ProviderContext::Recorded),
            invocations: Vec::new(),
        });
    } else {
        let value = snapshot.as_mut().expect("existing snapshot");
        if let Some(provider) = record.provider {
            value.provider = provider.decode()?;
        }
        if let Some(id) = record.provider_context {
            value.provider_context =
                ProviderContext::Recorded(ExecutionSessionId::new(id).map_err(corrupt)?);
        }
    }
    let value = snapshot.as_mut().expect("initialized snapshot");
    if record.queue_from != value.queue_history.len() {
        return Err(corrupt(
            "queue history must append to its complete previous prefix",
        ));
    }
    for reorder in record.queue_history {
        value.queue_history.push(reorder.decode()?);
    }
    value.invocations.truncate(record.invocation_count);
    let mut previous_index = None;
    for change in record.invocations {
        if change.index >= record.invocation_count
            || previous_index.is_some_and(|previous| previous >= change.index)
            || change.index > value.invocations.len()
        {
            return Err(corrupt(
                "journal invocation indices are not ordered and contiguous",
            ));
        }
        previous_index = Some(change.index);
        if change.index == value.invocations.len() {
            if change.events_from != 0 || change.scheduling_from != 0 {
                return Err(corrupt("new invocation cannot retain missing history"));
            }
            let mut invocation = change
                .metadata
                .ok_or_else(|| corrupt("new invocation has no metadata"))?
                .decode()?;
            for event in change.events {
                invocation.events.push(
                    event.decode(
                        value
                            .provider_context
                            .recorded()
                            .ok_or_else(|| corrupt("event requires recorded provider context"))?,
                        &invocation.request.execution_id,
                    )?,
                );
            }
            invocation.scheduling = change
                .scheduling
                .into_iter()
                .map(SchedulingEvent::decode)
                .collect::<Result<_, _>>()?;
            value.invocations.push(invocation);
        } else {
            let invocation = &mut value.invocations[change.index];
            if change.events_from > invocation.events.len()
                || change.scheduling_from > invocation.scheduling.len()
            {
                return Err(corrupt("journal tail starts beyond retained history"));
            }
            if let Some(metadata) = change.metadata {
                let mut replacement = metadata.decode()?;
                replacement.events = std::mem::take(&mut invocation.events);
                replacement.scheduling = std::mem::take(&mut invocation.scheduling);
                *invocation = replacement;
            }
            invocation.events.truncate(change.events_from);
            for event in change.events {
                invocation.events.push(
                    event.decode(
                        value
                            .provider_context
                            .recorded()
                            .ok_or_else(|| corrupt("event requires recorded provider context"))?,
                        &invocation.request.execution_id,
                    )?,
                );
            }
            invocation.scheduling.truncate(change.scheduling_from);
            for event in change.scheduling {
                invocation.scheduling.push(event.decode()?);
            }
        }
    }
    if value.invocations.len() != record.invocation_count {
        return Err(corrupt("journal record omitted new invocations"));
    }
    validate(value)?;
    Ok(())
}

/// Encode only changed metadata and the suffix after each common typed prefix.
pub(in crate::infrastructure::session_storage) fn encode(
    previous: Option<&SessionSnapshot>,
    value: &SessionSnapshot,
    sequence: u64,
) -> Result<Option<Vec<u8>>, StorageError> {
    let provider_changed = previous.is_none_or(|prior| prior.provider != value.provider);
    let session_changed =
        previous.is_none_or(|prior| prior.provider_context != value.provider_context);
    let mut changes = Vec::new();
    for (index, invocation) in value.invocations.iter().enumerate() {
        let prior = previous.and_then(|snapshot| snapshot.invocations.get(index));
        let events_from = prior.map_or(0, |prior| common_prefix(&prior.events, &invocation.events));
        let scheduling_from = prior.map_or(0, |prior| {
            common_prefix(&prior.scheduling, &invocation.scheduling)
        });
        let metadata_changed = prior.is_none_or(|prior| !same_metadata(prior, invocation));
        if !metadata_changed
            && prior.is_some_and(|prior| {
                prior.events.len() == events_from && prior.scheduling.len() == scheduling_from
            })
            && events_from == invocation.events.len()
            && scheduling_from == invocation.scheduling.len()
        {
            continue;
        }
        let metadata = metadata_changed.then(|| Metadata::from(invocation));
        changes.push(InvocationChange {
            index,
            metadata,
            events_from,
            events: invocation.events[events_from..]
                .iter()
                .cloned()
                .map(Event::from)
                .collect(),
            scheduling_from,
            scheduling: invocation.scheduling[scheduling_from..]
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
        });
    }
    let queue_from = previous.map_or(0, |prior| {
        common_prefix(&prior.queue_history, &value.queue_history)
    });
    if previous.is_some_and(|prior| queue_from != prior.queue_history.len()) {
        return Err(corrupt(
            "saved queue history cannot be replaced or truncated",
        ));
    }
    if changes.is_empty()
        && queue_from == value.queue_history.len()
        && previous.is_none_or(|prior| prior.queue_history.len() == queue_from)
        && !provider_changed
        && !session_changed
        && previous.is_some_and(|prior| prior.invocations.len() == value.invocations.len())
    {
        return Ok(None);
    }
    let record = Record {
        sequence,
        id: value.id.as_str().into(),
        provider: provider_changed.then(|| Provider {
            name: value.provider.name().into(),
            model_id: value.provider.model_id().into(),
            context: value.provider.context().into(),
        }),
        provider_context: session_changed
            .then(|| {
                value
                    .provider_context
                    .recorded()
                    .map(|id| id.as_str().into())
            })
            .flatten(),
        queue_from,
        queue_history: value.queue_history[queue_from..]
            .iter()
            .map(QueueEvent::from)
            .collect(),
        invocation_count: value.invocations.len(),
        invocations: changes,
    };
    let mut bytes = serde_json::to_vec(&record).map_err(corrupt)?;
    bytes.push(b'\n');
    Ok(Some(bytes))
}
fn common_prefix<T: PartialEq>(previous: &[T], next: &[T]) -> usize {
    previous
        .iter()
        .zip(next)
        .take_while(|(left, right)| left == right)
        .count()
}
fn same_metadata(left: &InvocationRecord, right: &InvocationRecord) -> bool {
    left.submission == right.submission
        && left.request == right.request
        && left.actor == right.actor
        && left.acknowledgement == right.acknowledgement
        && left.provider_report == right.provider_report
        && left.local_cancellation == right.local_cancellation
        && left.local_outcome == right.local_outcome
        && left.cancellation == right.cancellation
        && left.result == right.result
}
fn io_error(error: std::io::Error) -> StorageError {
    StorageError::Io(error.to_string())
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/session_storage/journal/decode_bounds.rs"]
mod decode_bounds_tests;
