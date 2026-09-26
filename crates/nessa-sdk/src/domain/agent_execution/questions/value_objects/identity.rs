//! The identity one ask is answered under.
use crate::domain::agent_execution::ExecutionError;

/// The longest question identity this retains.
const MAX_QUESTION_ID_BYTES: usize = 256;

/// What an answer is correlated to: one ask, within one execution.
///
/// Minted from the provider's own request identity, so an answer can be matched
/// to the question that is still waiting for it rather than to whichever ask
/// happens to be open.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QuestionId(Box<str>);

impl QuestionId {
    /// Take `value` as an identity, bounded and non-empty.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("question ID"));
        }
        if value.len() > MAX_QUESTION_ID_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "question ID",
                max_bytes: MAX_QUESTION_ID_BYTES,
            });
        }
        Ok(Self(value.into_boxed_str()))
    }

    /// Borrow the exact identity text without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
