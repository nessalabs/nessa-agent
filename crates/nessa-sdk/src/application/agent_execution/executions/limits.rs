//! Shared identity, chunk, and cumulative observation admission for live and restored evidence.
//! These application limits do not restrict the reusable domain identity types.
use super::ExecutionEvent;
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::executions::MessageChunk;

pub(crate) const MAX_OBSERVATION_ID_BYTES: usize = 256;

pub(crate) fn validate_observation_id(id: &str) -> Result<(), AgentError> {
    if id.len() > MAX_OBSERVATION_ID_BYTES {
        return Err(AgentError::InvalidInput(
            "execution identity exceeds binding limit".into(),
        ));
    }
    Ok(())
}

/// Maximum retained UTF-8 allocation for one text or thought observation.
pub(crate) const MAX_MESSAGE_CHUNK_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn validate_message_chunk(chunk: &MessageChunk) -> Result<(), AgentError> {
    if chunk.payload_bytes() > MAX_MESSAGE_CHUNK_BYTES {
        return Err(AgentError::Protocol(
            "message chunk exceeds the 4 MiB retained-byte limit".into(),
        ));
    }
    Ok(())
}

/// One invocation may retain more than the transient transport queue, but cannot
/// grow without bound when a fast consumer continuously drains that queue.
pub(crate) const MAX_RETAINED_OUTPUT_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MAX_RETAINED_OUTPUT_EVENTS: usize = 262_144;

#[derive(Clone, Copy, Default)]
pub(crate) struct ObservationUsage {
    bytes: usize,
    count: usize,
}
impl ObservationUsage {
    pub(crate) fn with_event(self, event: &ExecutionEvent) -> Result<Self, AgentError> {
        let next = Self {
            bytes: self.bytes.saturating_add(event.retained_bytes()),
            count: self.count.saturating_add(1),
        };
        if next.bytes > MAX_RETAINED_OUTPUT_BYTES || next.count > MAX_RETAINED_OUTPUT_EVENTS {
            return Err(AgentError::OutputRetentionLimit);
        }
        Ok(next)
    }
    pub(crate) fn from_events(events: &[ExecutionEvent]) -> Result<Self, AgentError> {
        events.iter().try_fold(Self::default(), Self::with_event)
    }
}

// Snapshot clones compact Vec capacity. Grow explicitly so a valid non-power-of-
// two length cannot double beyond the separately bounded spare-slot allowance.
pub(crate) fn reserve_observation_slot(events: &mut Vec<ExecutionEvent>) {
    if events.len() == events.capacity() {
        let capacity = events
            .len()
            .saturating_mul(2)
            .clamp(4, MAX_RETAINED_OUTPUT_EVENTS);
        events.reserve_exact(capacity.saturating_sub(events.len()));
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/executions/output_retention.rs"]
mod tests;
