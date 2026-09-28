//! Immutable correlation captured when an adapter admits a question.
#![deny(missing_docs)]

use super::{ExecutionEvent, ExecutionUpdate};
use crate::domain::agent_execution::executions::ExecutionId;
use crate::domain::agent_execution::questions::{AgentQuestion, QuestionId};
use crate::domain::agent_execution::sessions::ExecutionSessionId;

/// An admitted ask's provider session, execution, identity and original contents.
///
/// Obtain this value from [`super::ExecutionController::ask_question`]. Retain it
/// while the provider waits, publish [`event`](Self::event), and pass it to
/// [`crate::application::agent_execution::permissions::QuestionAnswerRecord`]
/// for answer or cancellation evidence. Clones retain the same admission; they
/// do not admit another ask or claim that an event or answer was delivered.
/// The controller reserves the ID until execution settlement; the adapter owns
/// open-question limits and resolution ordering.
/// This value performs no I/O and remains usable for evidence after teardown.
///
/// Private fields prevent replacing the original question independently:
/// ```compile_fail
/// use nessa_sdk::application::agent_execution::executions::AdmittedQuestion;
/// use nessa_sdk::domain::agent_execution::questions::AgentQuestion;
/// fn substitute(mut admitted: AdmittedQuestion, other: AgentQuestion) {
///     admitted.question = other;
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedQuestion {
    session_id: ExecutionSessionId,
    execution_id: ExecutionId,
    id: QuestionId,
    question: AgentQuestion,
}
impl AdmittedQuestion {
    pub(super) fn new(
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        id: QuestionId,
        question: AgentQuestion,
    ) -> Self {
        Self {
            session_id,
            execution_id,
            id,
            question,
        }
    }
    /// Provider context captured from the admitting controller.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Active execution validated at admission.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Adapter-minted identity of this ask.
    pub fn id(&self) -> &QuestionId {
        &self.id
    }
    /// Original question against which choices must be validated.
    pub fn question(&self) -> &AgentQuestion {
        &self.question
    }
    /// Project the admitted ask for publication; this does not publish it.
    pub fn event(&self) -> ExecutionEvent {
        ExecutionEvent::new(
            self.execution_id.clone(),
            ExecutionUpdate::QuestionAsked {
                id: self.id.clone(),
                question: self.question.clone(),
            },
        )
    }
    /// Project closure of this ask, including after its execution has settled.
    /// The adapter decides when closure occurs; this performs no state change or I/O.
    pub fn closed_event(&self) -> ExecutionEvent {
        ExecutionEvent::new(
            self.execution_id.clone(),
            ExecutionUpdate::QuestionClosed {
                id: self.id.clone(),
            },
        )
    }
}
