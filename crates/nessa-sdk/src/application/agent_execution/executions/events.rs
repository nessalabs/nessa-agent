//! Owned, execution-correlated observations for storage and live projections.
#![deny(missing_docs)]

use super::{
    limits::{validate_message_chunk, validate_observation_id, MAX_MESSAGE_CHUNK_BYTES},
    ExecutionController,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::{
    permissions::{CancellationOrigin, DeclinedReview, PermissionCancellation},
    tools::ToolReviewInput,
};
use crate::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, MessageChunk},
    permissions::{
        PermissionEffect, PermissionId, PermissionOptions, PermissionRequest,
        PermissionReviewDeadline,
    },
    questions::{AgentQuestion, QuestionId},
    tools::{ToolCall, ToolCallId, ToolCallUpdate, ToolObservation},
};

use std::mem::size_of;

/// An immutable observation correlated to one execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionEvent {
    execution_id: ExecutionId,
    update: ExecutionUpdate,
    native_hooks: Option<bool>,
    observed_at_ms: Option<u64>,
    /// Set by the session when it queues the report. A later read keeps
    /// `observed_at_ms`. A time the provider wrote has this clear, and the
    /// session replaces it.
    session_arrival: bool,
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
                deadline: _,
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
            // A permission identity is bounded only where it is checked, and
            // saved history checks this one, so it is checked here too.
            ExecutionUpdate::DeadlineAnswered { id, .. } => validate_observation_id(id.as_str()),
            // An ask is bounded by its own construction — every text and both
            // collections — so what is left to check is the identity beside it.
            ExecutionUpdate::QuestionClosed { id } => validate_observation_id(id.as_str()),
            ExecutionUpdate::QuestionAsked { id, question } => {
                validate_observation_id(id.as_str())?;
                ExecutionController::validate_tool_payload(
                    question
                        .payload_bytes()
                        .saturating_add(self.execution_id.as_str().len())
                        .saturating_add(id.as_str().len()),
                )
            }
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
                deadline,
            } => id
                .as_str()
                .len()
                .saturating_add(tool_id.as_str().len())
                .saturating_add(observation.payload_bytes())
                .saturating_add(input.name.capacity())
                .saturating_add(input.arguments_json.capacity())
                .saturating_add(options.payload_bytes())
                .saturating_add(
                    deadline
                        .as_ref()
                        .map_or(0, |deadline| deadline.reason().len()),
                ),
            ExecutionUpdate::PermissionCancelled(record) => {
                let actor = match record.origin() {
                    CancellationOrigin::Client(actor) => actor
                        .principal_id()
                        .len()
                        .saturating_add(actor.surface_id().len())
                        .saturating_add(actor.request_id().len()),
                    CancellationOrigin::Deadline(deadline) => deadline.reason().len(),
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
            ExecutionUpdate::ReviewDeclined(review) => review
                .id()
                .as_str()
                .len()
                .saturating_add(review.decline().declared().map_or(0, str::len))
                .saturating_add(review.rule_bytes()),
            ExecutionUpdate::QuestionAsked { id, question } => id
                .as_str()
                .len()
                .saturating_add(question.allocation_bytes()),
            ExecutionUpdate::QuestionClosed { id } => id.as_str().len(),
            ExecutionUpdate::DeadlineAnswered { id, reason, .. } => {
                id.as_str().len().saturating_add(reason.as_str().len())
            }
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
            native_hooks: None,
            observed_at_ms: None,
            session_arrival: false,
        }
    }
    /// The same observation, stamped with the hooks choice of the process start
    /// that produced it. The stamp is saved with the event, so a reader after a
    /// restart sees the choice the call ran under, not the one in effect now.
    pub fn with_native_hooks(mut self, allowed: bool) -> Self {
        self.native_hooks = Some(allowed);
        self
    }
    /// Whether the producing process start allowed the harness's own hooks.
    /// `None` when that start had no such choice, or the event was saved
    /// without one.
    pub fn native_hooks(&self) -> Option<bool> {
        self.native_hooks
    }
    /// The same observation, stamped with the wall-clock time the session
    /// received it, in Unix milliseconds. The stamp is saved with the event,
    /// so when each report arrived — a tool's start and end among them —
    /// survives a restart.
    pub fn with_observed_at_ms(mut self, at_ms: u64) -> Self {
        self.observed_at_ms = Some(at_ms);
        self
    }
    /// The same observation, stamped when the session queued it.
    ///
    /// `at_ms` is the process timeline at that queue. A later read keeps it.
    /// This is not a provider's time: [`Self::with_observed_at_ms`] leaves
    /// the arrival mark clear so the session can replace that time.
    pub(crate) fn stamped_at_arrival(mut self, at_ms: u64) -> Self {
        self.observed_at_ms = Some(at_ms);
        self.session_arrival = true;
        self
    }
    /// Whether the session queued this report and stamped that time.
    pub(crate) fn session_arrival(&self) -> bool {
        self.session_arrival
    }
    /// When the session received this observation, in Unix milliseconds.
    /// The session stamps every report as it arrives; `None` is an event
    /// built outside a session, or one saved before stamps were kept.
    pub fn observed_at_ms(&self) -> Option<u64> {
        self.observed_at_ms
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
    /// A stop carries the rule that required it. A continue carries none.
    ReviewDeclined(DeclinedReview),
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
        /// When this review answers itself if nobody does, and as what.
        /// Absent means the review waits for a person.
        deadline: Option<PermissionReviewDeadline>,
    },
    /// The review's deadline passed and selected `effect`.
    ///
    /// Selecting a refusal that stops the turn is still this event; the reason
    /// says so. The turn continues when the selected option does not stop it.
    ///
    /// This is the lasting fact a surface reads after the pending review is
    /// gone. It is not a second decision: the answer itself is the audit record.
    DeadlineAnswered {
        /// The review that the deadline answered.
        id: PermissionId,
        /// The outcome the deadline took.
        effect: PermissionEffect,
        /// The reason the deadline recorded, already bounded by construction.
        reason: crate::domain::agent_execution::permissions::DeadlineReason,
    },
    /// A question the agent put to a person, waiting on an answer.
    ///
    /// Not a review: nothing is being authorised, and an answer is the agent's
    /// own input rather than permission to act.
    QuestionAsked {
        /// Ask identity, scoped to this execution, that an answer names.
        id: QuestionId,
        /// What was asked and what will be accepted as an answer.
        question: AgentQuestion,
    },
    /// An ask that is no longer waiting, because it was answered or withdrawn.
    ///
    /// Carried as an observation rather than left to whoever answered, so a
    /// surface that was not the one answering — or one reading the session back
    /// afterwards — sees the question stop being open.
    QuestionClosed {
        /// The ask that is no longer waiting.
        id: QuestionId,
    },
}
