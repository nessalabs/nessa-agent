//! Mandatory permission evidence, independent of live event consumers.
#![deny(missing_docs)]

use super::{ActionContext, PermissionResolution};
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::executions::ExecutionId;
use crate::domain::agent_execution::permissions::{ReviewDecline, ReviewDeclineId};
use crate::domain::agent_execution::questions::{
    QuestionId, QuestionRefusalReason, QuestionResponse,
};
use crate::domain::agent_execution::sessions::ExecutionSessionId;

/// Observed delivery stage of an already selected permission answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionAnswerDelivery {
    /// Local decision recorded before any response write is attempted.
    Selected,
    /// Response write completed. This is not provider acknowledgement or a tool effect.
    Written,
    /// Response write failed or timed out; delivery and tool effects may be uncertain.
    Failed(AgentError),
}

/// Immutable answer evidence retaining correlation, exact input, decision, and actor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionAnswerRecord {
    resolution: PermissionResolution,
    delivery: PermissionAnswerDelivery,
}
impl PermissionAnswerRecord {
    /// Creates immutable delivery evidence for `resolution`, retaining its
    /// controller-owned session, validated decision, original input, and actor.
    /// `delivery` describes the local selection or observed write result.
    /// This performs no I/O and does not claim provider acknowledgement. The
    /// owning session cannot be supplied separately or relabeled by an adapter.
    pub fn new(resolution: PermissionResolution, delivery: PermissionAnswerDelivery) -> Self {
        Self {
            resolution,
            delivery,
        }
    }
    /// Provider context containing the affected execution and permission.
    pub fn session_id(&self) -> &ExecutionSessionId {
        self.resolution.session_id()
    }
    /// Original input, offered options, selected decision, and verified attribution.
    pub fn resolution(&self) -> &PermissionResolution {
        &self.resolution
    }
    /// Local selection or observed wire-delivery result; never proof of tool execution.
    pub fn delivery(&self) -> &PermissionAnswerDelivery {
        &self.delivery
    }
}

/// Immutable evidence that a review was refused before a host could be offered it.
///
/// A decline is an answer the binding gave on its own: the agent asked to use a
/// tool and was told no, without anybody being shown a choice. That is worth
/// the same evidence as an answered review, and it needs the same two facts
/// kept apart — what was decided locally, and what the wire did with it.
/// [`PermissionAnswerDelivery`] carries the second, because a refusal is
/// delivered exactly the way a selection is and a reader should not have to
/// learn two vocabularies for one thing.
///
/// There is no permission identity here, and its absence is the point: the
/// request never became one. Correlation is the execution the agent was working
/// on when it asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewDeclineRecord {
    session_id: ExecutionSessionId,
    execution_id: ExecutionId,
    id: ReviewDeclineId,
    decline: ReviewDecline,
    delivery: PermissionAnswerDelivery,
}
impl ReviewDeclineRecord {
    /// Record that `decline` was decided for `execution_id` within `session_id`.
    ///
    /// The constructor consumes the provider `session_id`, owning `execution_id`,
    /// execution-scoped decline `id`, immutable `decline`, and observed
    /// `delivery` stage. It performs no I/O and cannot fail because each domain
    /// value was validated before reaching this application boundary.
    ///
    /// `delivery` describes this binding's own progress — [`Selected`] before a
    /// response is written, then [`Written`] or [`Failed`] once the write has
    /// been observed. It never claims the provider accepted the refusal or that
    /// the tool did not run; an agent that is told no has still been told
    /// something, and what it does next is its own.
    ///
    /// The audit producer must emit `Selected` first and reuse the same `id` and
    /// `decline` for the later delivery record; this immutable record preserves
    /// that correlation but does not read prior audit history itself.
    ///
    /// [`Selected`]: PermissionAnswerDelivery::Selected
    /// [`Written`]: PermissionAnswerDelivery::Written
    /// [`Failed`]: PermissionAnswerDelivery::Failed
    pub fn new(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        id: ReviewDeclineId,
        decline: ReviewDecline,
        delivery: PermissionAnswerDelivery,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            id,
            decline,
            delivery,
        }
    }
    /// Provider session whose agent was refused.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Execution the agent was running when it asked for the tool.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Stable identity pairing the selected and delivery-stage audit records.
    pub fn id(&self) -> &ReviewDeclineId {
        &self.id
    }
    /// Which tool was refused, where it could be named, and why.
    pub fn decline(&self) -> &ReviewDecline {
        &self.decline
    }
    /// Local decision or observed write result; never provider acknowledgement.
    pub fn delivery(&self) -> &PermissionAnswerDelivery {
        &self.delivery
    }
}

/// Immutable evidence that an agent's question was answered.
///
/// Not a permission answer: nothing was authorised, and declining is an answer
/// rather than a refusal. It keeps the same two facts apart for the same
/// reason — what was decided here, and what the wire did with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionAnswerRecord {
    session_id: ExecutionSessionId,
    execution_id: ExecutionId,
    question_id: QuestionId,
    response: QuestionResponse,
    actor: Option<ActionContext>,
    delivery: PermissionAnswerDelivery,
}
impl QuestionAnswerRecord {
    /// Record that `response` ended `question_id` within `session_id`.
    ///
    /// `actor` is who answered, verified by the host that took the answer.
    /// It is present exactly when somebody chose the outcome — an answer or a
    /// decline — and absent for a cancellation, which nobody chose: saying who
    /// did would be inventing an initiator, and leaving one out of an explicit
    /// answer would lose the one fact an audit of it exists to keep.
    pub fn new(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        question_id: QuestionId,
        response: QuestionResponse,
        actor: Option<ActionContext>,
        delivery: PermissionAnswerDelivery,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            question_id,
            response,
            actor,
            delivery,
        }
    }
    /// Who answered, where somebody did; `None` when the ask was cancelled.
    pub fn actor(&self) -> Option<&ActionContext> {
        self.actor.as_ref()
    }
    /// Provider session whose agent asked.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Execution the agent was running when it asked.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// The ask this answers.
    pub fn question_id(&self) -> &QuestionId {
        &self.question_id
    }
    /// What was answered, validated against what was asked.
    pub fn response(&self) -> &QuestionResponse {
        &self.response
    }
    /// Local decision or observed write result; never provider acknowledgement.
    pub fn delivery(&self) -> &PermissionAnswerDelivery {
        &self.delivery
    }
}

/// Immutable evidence that an agent's question was refused before anybody saw it.
///
/// The counterpart of [`ReviewDeclineRecord`] for asks, and for the same reason:
/// the agent is told `cancel` and abandons the tool call that asked, so the
/// refusal is a decision with an effect, and a reader must be able to tell it
/// from a person declining. There is no question identity, because the ask
/// never became one; correlation is the execution the agent was running. No
/// initiator either: this binding decided, which the record says by having no
/// actor rather than by naming one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionRefusalRecord {
    session_id: ExecutionSessionId,
    execution_id: ExecutionId,
    reason: QuestionRefusalReason,
    delivery: PermissionAnswerDelivery,
}
impl QuestionRefusalRecord {
    /// Record that an ask from `execution_id` within `session_id` was refused
    /// for `reason`.
    ///
    /// `delivery` is this binding's own progress — [`Selected`] before the
    /// refusal is written, then [`Written`] or [`Failed`] once the write has
    /// been observed. It never claims the provider acted on the refusal.
    ///
    /// [`Selected`]: PermissionAnswerDelivery::Selected
    /// [`Written`]: PermissionAnswerDelivery::Written
    /// [`Failed`]: PermissionAnswerDelivery::Failed
    pub fn new(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        reason: QuestionRefusalReason,
        delivery: PermissionAnswerDelivery,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            reason,
            delivery,
        }
    }
    /// Provider session whose agent asked.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Execution the agent was running when it asked.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Which limit of this binding the ask ran into.
    pub fn reason(&self) -> QuestionRefusalReason {
        self.reason
    }
    /// Local decision or observed write result; never provider acknowledgement.
    pub fn delivery(&self) -> &PermissionAnswerDelivery {
        &self.delivery
    }
}
