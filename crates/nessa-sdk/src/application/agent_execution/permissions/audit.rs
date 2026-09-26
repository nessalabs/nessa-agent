//! Mandatory permission evidence, independent of live event consumers.
#![deny(missing_docs)]

use super::{ActionContext, PermissionResolution};
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::executions::ExecutionId;
use crate::domain::agent_execution::permissions::{ReviewDecline, ReviewDeclineId};
use crate::domain::agent_execution::questions::{
    AcceptedAnswer, AgentQuestion, QuestionCancellation, QuestionChoice, QuestionId,
    QuestionRefusalReason, QuestionResponse,
};
use crate::domain::agent_execution::questions::{MAX_OPEN_ASK_COST, MAX_OPEN_QUESTIONS};
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::domain::agent_execution::ExecutionError;

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

/// Immutable evidence that an agent's question was answered, or ended unanswered.
///
/// Not a permission answer: nothing was authorised, and declining is an answer
/// rather than a refusal. It keeps the same two facts apart for the same
/// reason — what was decided here, and what the wire did with it.
///
/// It keeps the ask itself, as a permission record keeps its request and
/// input: what was offered, what was required, and where own words go. Without
/// it the evidence could not show on its own that a recorded choice was one the
/// question offered, or what a decline or cancellation left unanswered.
///
/// Who initiated it is decided by how it is built, not checked afterwards:
/// [`chosen`](Self::chosen) takes the verified answerer, and
/// [`ended`](Self::ended) takes none, because nobody chose a cancellation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionAnswerRecord {
    session_id: ExecutionSessionId,
    execution_id: ExecutionId,
    question_id: QuestionId,
    question: AgentQuestion,
    response: QuestionResponse,
    actor: Option<ActionContext>,
    delivery: PermissionAnswerDelivery,
}
impl QuestionAnswerRecord {
    /// Record that `actor` answered `question_id` with `choices`, or declined
    /// it where `choices` is `None`.
    ///
    /// `actor` is who answered, verified by the host that took the answer;
    /// leaving it out of an explicit answer would lose the one fact an audit
    /// of it exists to keep. `choices` are validated here against `question`,
    /// the ask as it was asked, so the record can only ever hold an answer to
    /// the question it holds. Returns the domain's reason when they disagree.
    pub fn chosen(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        question_id: QuestionId,
        question: AgentQuestion,
        choices: Option<Vec<QuestionChoice>>,
        actor: ActionContext,
        delivery: PermissionAnswerDelivery,
    ) -> Result<Self, ExecutionError> {
        let response = match choices {
            Some(choices) => QuestionResponse::Answered(AcceptedAnswer::new(&question, choices)?),
            None => QuestionResponse::Declined,
        };
        Ok(Self {
            session_id,
            execution_id,
            question_id,
            question,
            response,
            actor: Some(actor),
            delivery,
        })
    }
    /// The same decision at a later stage of its delivery.
    ///
    /// Everything but `delivery` is kept, so the written record of an answer
    /// can only be the record of the answer that was selected.
    pub fn with_delivery(self, delivery: PermissionAnswerDelivery) -> Self {
        Self { delivery, ..self }
    }
    /// Record that `question_id` ended unanswered, for `cause`.
    ///
    /// There is no actor: nobody chose this, and naming one would invent an
    /// initiator. `cause` says what ended it instead.
    pub fn ended(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        question_id: QuestionId,
        question: AgentQuestion,
        cause: QuestionCancellation,
        delivery: PermissionAnswerDelivery,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            question_id,
            question,
            response: QuestionResponse::Cancelled(cause),
            actor: None,
            delivery,
        }
    }
    /// Who answered; present exactly when the response is an answer or a
    /// decline, and `None` when the ask was cancelled.
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
    /// The ask as it was asked: what it offered, required and invited.
    pub fn question(&self) -> &AgentQuestion {
        &self.question
    }
    /// What was answered, validated against [`question`](Self::question).
    pub fn response(&self) -> &QuestionResponse {
        &self.response
    }
    /// Local decision or observed write result; never provider acknowledgement.
    pub fn delivery(&self) -> &PermissionAnswerDelivery {
        &self.delivery
    }
}

/// What was open when an ask was refused for want of room, and the ask itself.
///
/// A refusal for room is a comparison, and a comparison is only evidence with
/// both sides of it: the ask that was read and would not fit, and what was
/// already open when it arrived. The limits it was held to are the published
/// [`MAX_OPEN_QUESTIONS`] and [`MAX_OPEN_ASK_COST`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedAsk {
    ask: AgentQuestion,
    open_asks: usize,
    open_cost: usize,
}
impl RefusedAsk {
    /// `ask` was refused while `open_asks` asks costing `open_cost` together
    /// were already open.
    ///
    /// Only accounting that could describe asks a binding actually held is
    /// accepted: never more open than [`MAX_OPEN_QUESTIONS`], never costing
    /// more than [`MAX_OPEN_ASK_COST`] — each was admitted within it — and no
    /// cost without an ask or an ask without a cost, since every ask costs
    /// something to carry. Anything else is
    /// [`ExecutionError::InvalidQuestionRefusal`]: evidence of a state that
    /// never existed.
    pub fn new(
        ask: AgentQuestion,
        open_asks: usize,
        open_cost: usize,
    ) -> Result<Self, ExecutionError> {
        if open_asks > MAX_OPEN_QUESTIONS
            || open_cost > MAX_OPEN_ASK_COST
            || (open_asks == 0) != (open_cost == 0)
        {
            return Err(ExecutionError::InvalidQuestionRefusal);
        }
        Ok(Self {
            ask,
            open_asks,
            open_cost,
        })
    }
    /// The ask as it arrived.
    pub fn ask(&self) -> &AgentQuestion {
        &self.ask
    }
    /// Take back the ask, once there turned out to be room for it.
    pub fn into_ask(self) -> AgentQuestion {
        self.ask
    }
    /// How many asks were already open.
    pub fn open_asks(&self) -> usize {
        self.open_asks
    }
    /// What the asks already open cost together to carry.
    pub fn open_cost(&self) -> usize {
        self.open_cost
    }
    /// Why there was no room for the ask, or `None` if there was.
    ///
    /// The one place the comparison is made: the binding decides a refusal
    /// with it, and a record takes its reason from it, so a record can never
    /// hold a reason its own evidence contradicts.
    pub fn reason(&self) -> Option<QuestionRefusalReason> {
        if self.open_asks >= MAX_OPEN_QUESTIONS {
            Some(QuestionRefusalReason::TooManyOpen)
        } else if self.open_cost.saturating_add(self.ask.carrying_cost()) > MAX_OPEN_ASK_COST {
            Some(QuestionRefusalReason::TooLarge)
        } else {
            None
        }
    }
}

/// Immutable evidence that an agent's question was refused before anybody saw it.
///
/// The counterpart of [`ReviewDeclineRecord`] for asks, and for the same reason:
/// the agent is told `cancel` and abandons the tool call that asked, so the
/// refusal is a decision with an effect, and a reader must be able to tell it
/// from a person declining. The refused request is given an identity of its
/// own, as a declined review is, so its decision and its write pair up even
/// when several asks are refused in one execution or one of the records is
/// missing. No initiator: this binding decided, which the record says by
/// having no actor rather than by naming one.
///
/// A refusal for room — too many open, or too large beside them — carries the
/// [`RefusedAsk`] it was decided on and takes its reason from it; every other
/// refusal happens before there is an ask to keep, and carries none. Which
/// constructor built it is what holds the two to that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionRefusalRecord {
    session_id: ExecutionSessionId,
    execution_id: ExecutionId,
    id: QuestionId,
    reason: QuestionRefusalReason,
    refused: Option<RefusedAsk>,
    delivery: PermissionAnswerDelivery,
}
impl QuestionRefusalRecord {
    /// Record that ask `id`, from `execution_id` within `session_id`, was
    /// refused for `reason` before it could be read or admitted at all.
    ///
    /// For [`Unsupported`], [`UnreadableQuestion`] and [`SessionEnding`]; a
    /// refusal for room is recorded by [`for_room`](Self::for_room), which
    /// keeps its evidence, and is refused here as
    /// [`ExecutionError::InvalidQuestionRefusal`]. `id` is minted for the
    /// refused request, in the same sequence as the asks that were admitted.
    /// `delivery` is this binding's own progress — [`Selected`] before the
    /// refusal is written, then [`Written`] or [`Failed`] once the write has
    /// been observed. It never claims the provider acted on the refusal.
    ///
    /// [`Unsupported`]: QuestionRefusalReason::Unsupported
    /// [`UnreadableQuestion`]: QuestionRefusalReason::UnreadableQuestion
    /// [`SessionEnding`]: QuestionRefusalReason::SessionEnding
    /// [`Selected`]: PermissionAnswerDelivery::Selected
    /// [`Written`]: PermissionAnswerDelivery::Written
    /// [`Failed`]: PermissionAnswerDelivery::Failed
    pub fn new(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        id: QuestionId,
        reason: QuestionRefusalReason,
        delivery: PermissionAnswerDelivery,
    ) -> Result<Self, ExecutionError> {
        if matches!(
            reason,
            QuestionRefusalReason::TooManyOpen | QuestionRefusalReason::TooLarge
        ) {
            return Err(ExecutionError::InvalidQuestionRefusal);
        }
        Ok(Self {
            session_id,
            execution_id,
            id,
            reason,
            refused: None,
            delivery,
        })
    }
    /// Record that ask `id` was refused for want of room, on `refused`.
    ///
    /// The reason is taken from the evidence, not given beside it: evidence
    /// that shows there was room is refused as
    /// [`ExecutionError::InvalidQuestionRefusal`], because it records no
    /// refusal at all. Otherwise as [`new`](Self::new).
    pub fn for_room(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        id: QuestionId,
        refused: RefusedAsk,
        delivery: PermissionAnswerDelivery,
    ) -> Result<Self, ExecutionError> {
        let reason = refused
            .reason()
            .ok_or(ExecutionError::InvalidQuestionRefusal)?;
        Ok(Self {
            session_id,
            execution_id,
            id,
            reason,
            refused: Some(refused),
            delivery,
        })
    }
    /// The same refusal at a later stage of its delivery.
    pub fn with_delivery(self, delivery: PermissionAnswerDelivery) -> Self {
        Self { delivery, ..self }
    }
    /// Provider session whose agent asked.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Execution the agent was running when it asked.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// The identity minted for the refused request; the same on its decision
    /// and on its write.
    pub fn id(&self) -> &QuestionId {
        &self.id
    }
    /// Which limit of this binding the ask ran into.
    pub fn reason(&self) -> QuestionRefusalReason {
        self.reason
    }
    /// The ask, and what was open, for a refusal made for room.
    pub fn refused(&self) -> Option<&RefusedAsk> {
        self.refused.as_ref()
    }
    /// Local decision or observed write result; never provider acknowledgement.
    pub fn delivery(&self) -> &PermissionAnswerDelivery {
        &self.delivery
    }
}
