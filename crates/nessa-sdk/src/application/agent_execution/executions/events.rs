//! Owned, execution-correlated observations for storage and live projections.
#![deny(missing_docs)]

use super::{
    limits::{validate_message_chunk, validate_observation_id, MAX_MESSAGE_CHUNK_BYTES},
    ExecutionController,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::{
    permissions::{CancellationOrigin, PermissionCancellation},
    tools::ToolReviewInput,
};
use crate::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, MessageChunk},
    permissions::{PermissionId, PermissionOptions, PermissionRequest, ReviewDeclineObservation},
    tools::{ToolCall, ToolCallId, ToolCallUpdate, ToolObservation},
};

use std::mem::size_of;

/// An immutable observation correlated to one execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionEvent {
    execution_id: ExecutionId,
    update: ExecutionUpdate,
}
impl ExecutionEvent {
    /// Maximum retained UTF-8 allocation of one text or thought chunk (4 MiB).
    /// Adapters and snapshot admission reject larger allocations before copying
    /// or saving them. Domain construction already discards spare capacity; this
    /// is independent of token estimates and does not bound accumulated history.
    pub const MAX_MESSAGE_CHUNK_BYTES: usize = MAX_MESSAGE_CHUNK_BYTES;

    // Stateless borrowed admission must precede every event copy. Aggregate
    // history validation still enforces retained totals and review lifetimes.
    pub(crate) fn validate_payload_size(&self) -> Result<(), AgentError> {
        match &self.update {
            ExecutionUpdate::Finished(_) => Ok(()),
            ExecutionUpdate::Message(chunk) => validate_message_chunk(chunk),
            ExecutionUpdate::Tool(tool) => {
                validate_observation_id(tool.id().as_str())?;
                ExecutionController::validate_tool_payload(
                    ToolCall::initial_payload_bytes(&self.execution_id, tool)
                        .saturating_add(tool.id().as_str().len()),
                )
            }
            ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                observation,
                input,
                options,
            } => {
                ExecutionController::validate_review_payload(
                    &self.execution_id,
                    id,
                    tool_id,
                    input,
                    options,
                )?;
                ExecutionController::validate_tool_payload(
                    observation
                        .payload_bytes()
                        .saturating_add(self.execution_id.as_str().len())
                        .saturating_add(tool_id.as_str().len().saturating_mul(2)),
                )
            }
            ExecutionUpdate::PermissionCancelled(record) => {
                let request = record.request();
                ExecutionController::validate_review_payload(
                    request.execution_id(),
                    request.id(),
                    request.tool_id(),
                    record.input(),
                    request.options(),
                )?;
                Ok(())
            }
            ExecutionUpdate::ReviewDeclined(_) => Ok(()),
        }
    }

    /// Account for one retained event, including its slot, identity, owned payload
    /// capacities and conservatively charged shared cancellation evidence.
    /// Vector spare slots and allocator overhead are separate.
    pub(crate) fn retained_bytes(&self) -> usize {
        let payload = match self.update() {
            ExecutionUpdate::Finished(_) => 0,
            ExecutionUpdate::Message(chunk) => chunk.payload_bytes(),
            ExecutionUpdate::Tool(tool) => tool
                .id()
                .as_str()
                .len()
                .saturating_add(tool.payload_bytes()),
            ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                observation,
                input,
                options,
            } => id
                .as_str()
                .len()
                .saturating_add(tool_id.as_str().len())
                .saturating_add(observation.payload_bytes())
                .saturating_add(input.name.capacity())
                .saturating_add(input.arguments_json.capacity())
                .saturating_add(options.payload_bytes()),
            ExecutionUpdate::PermissionCancelled(record) => {
                let actor = match record.origin() {
                    CancellationOrigin::Client(actor) => actor
                        .principal_id()
                        .len()
                        .saturating_add(actor.surface_id().len())
                        .saturating_add(actor.request_id().len()),
                    CancellationOrigin::Provider | CancellationOrigin::Runtime => 0,
                };
                record
                    .session_id()
                    .as_str()
                    .len()
                    .saturating_add(size_of::<PermissionRequest>())
                    .saturating_add(2 * size_of::<usize>()) // Arc's strong/weak counters.
                    .saturating_add(record.request().payload_bytes())
                    .saturating_add(record.input().name.capacity())
                    .saturating_add(record.input().arguments_json.capacity())
                    .saturating_add(actor)
            }
            ExecutionUpdate::ReviewDeclined(observation) => observation
                .id()
                .as_str()
                .len()
                .saturating_add(observation.decline().declared().map_or(0, str::len)),
        };
        size_of::<Self>()
            .saturating_add(self.execution_id().as_str().len())
            .saturating_add(payload)
    }

    /// Take ownership of `update` and its producing `execution_id`. Performs no I/O;
    /// adapters must supply the identity of the execution that produced this fact.
    /// This constructs a DTO; controller and session admission validate resource
    /// limits before the observation becomes authoritative or is persisted.
    pub fn new(execution_id: ExecutionId, update: ExecutionUpdate) -> Self {
        Self {
            execution_id,
            update,
        }
    }
    /// Borrow the stable identity used to route this observation.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Borrow the immutable observation without consuming its envelope.
    pub fn update(&self) -> &ExecutionUpdate {
        &self.update
    }
    /// Consume the envelope and return its owned observation payload.
    pub fn into_update(self) -> ExecutionUpdate {
        self.update
    }
}

/// Application projections combine domain observations with tool review metadata.
/// Finished follows the execution's output; delivery failures are port errors.
/// No message, tool update, new review, or second Finished may follow it. Later
/// cancellation evidence may still record cleanup of a previously pending review;
/// that evidence does not resume provider execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionUpdate {
    /// Provider-reported outcome after its output; Agent checks agreement with settlement.
    Finished(ExecutionOutcome),
    /// Exact streamed text or thought fragment.
    Message(MessageChunk),
    /// Sparse tool observation; omitted fields retain their previous meaning.
    Tool(ToolCallUpdate),
    /// Once-only cancellation evidence; independent audit remains mandatory.
    PermissionCancelled(PermissionCancellation),
    /// Runtime-owned refusal of a review that never became actionable.
    ///
    /// Repeated observations with the same `id` advance one local fact from
    /// selection to its wire-write result. They do not describe provider
    /// acknowledgement, tool execution, or a terminal execution state.
    ReviewDeclined(ReviewDeclineObservation),
    /// Pending review with the exact offered choices and captured tool input.
    PermissionRequested {
        /// Review identity, scoped to this execution.
        id: PermissionId,
        /// Tool whose proposed action requires a decision.
        tool_id: ToolCallId,
        /// Immutable tool state captured when the review was requested.
        observation: ToolObservation,
        /// Exact proposed action supplied for review and audit.
        input: ToolReviewInput,
        /// Validated choices offered for this request; no authority is granted by display.
        options: PermissionOptions,
    },
}
