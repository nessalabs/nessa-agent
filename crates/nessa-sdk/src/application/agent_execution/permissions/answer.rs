use super::{ActionContext, ApprovalAttribution};
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::{
    executions::ExecutionId,
    permissions::{PermissionId, PermissionOptionId},
    questions::{QuestionChoice, QuestionId},
};
use std::{fmt, future::Future, pin::Pin};

/// Whether an answer failure happened before or after the domain review was consumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionSelectionState {
    /// The exact review was rejected before selecting its option and remains pending.
    Pending,
    /// The domain selected the option; audit or wire delivery subsequently failed.
    Consumed,
    /// Interruption prevented the SDK from proving either state; reload authoritative state.
    Unknown,
}

/// Failed answer with selection state kept separate from its diagnostic cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionAnswerFailure {
    error: AgentError,
    selection: PermissionSelectionState,
}

impl PermissionAnswerFailure {
    pub(crate) fn new(error: AgentError, selection: PermissionSelectionState) -> Self {
        Self { error, selection }
    }
    /// Diagnostic failure; never infer review state from its variant or message.
    pub fn error(&self) -> &AgentError {
        &self.error
    }
    /// Authoritative knowledge of whether the reviewed option was selected.
    pub fn selection(&self) -> PermissionSelectionState {
        self.selection
    }
    /// Split the diagnostic and selection fact for application error mapping.
    pub fn into_parts(self) -> (AgentError, PermissionSelectionState) {
        (self.error, self.selection)
    }
}
impl fmt::Display for PermissionAnswerFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}
impl std::error::Error for PermissionAnswerFailure {}

/// Result of an attributed permission answer.
pub type PermissionAnswerResult = Result<super::PermissionResolution, PermissionAnswerFailure>;

/// Owned permission-answer operation that preserves selection state on failure.
pub type PermissionAnswerFuture<'a> =
    Pin<Box<dyn Future<Output = PermissionAnswerResult> + Send + 'a>>;

/// Host-authorized selection of one offered option for an exact pending review.
/// This command is not an execution retry or proof of tool delivery. The Agent
/// checks correlation through its provider; the adapter audits selection and write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionAnswer {
    /// Host-verified actor and decision basis. Never deserialize untrusted claims directly.
    pub attribution: ApprovalAttribution,
    /// Execution that owns the review; stale executions must not resolve later reviews.
    pub execution_id: ExecutionId,
    /// Pending permission identity within that execution.
    pub id: PermissionId,
    /// Exact offered option identity; arbitrary or previously resolved choices fail.
    pub option_id: PermissionOptionId,
}

/// What a host answers one of the agent's own questions with.
///
/// The choices are checked against the ask they name when the session resolves
/// them, not here: this carries a host's intent, and what the agent asked for
/// is the session's to know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionAnswer {
    /// Host-verified caller who answered. Carried to the audit record, because
    /// an explicit answer without its initiator is not evidence of anything.
    pub actor: ActionContext,
    /// Execution that owns the ask; a stale execution must not answer a later one.
    pub execution_id: ExecutionId,
    /// The ask being answered.
    pub id: QuestionId,
    /// What was chosen, or `None` to answer nothing at all.
    pub choices: Option<Vec<QuestionChoice>>,
}
