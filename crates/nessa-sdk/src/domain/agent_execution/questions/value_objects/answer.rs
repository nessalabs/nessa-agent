//! What a person answered, checked against what the agent actually asked.
use super::{AgentQuestion, AnswerShape, MAX_TEXT_BYTES};
use crate::domain::agent_execution::ExecutionError;

/// What was chosen for one question.
///
/// `values` are option values the question offered; `own_words` is prose typed
/// instead of, or beside, a choice. Both may be empty: a question nobody
/// answered is skipped, which every ask permits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionChoice {
    key: Box<str>,
    values: Vec<Box<str>>,
    own_words: Option<Box<str>>,
}

impl QuestionChoice {
    /// Choose `values` for the question at `key`, optionally in `own_words`.
    pub fn new(
        key: impl Into<String>,
        values: Vec<String>,
        own_words: Option<String>,
    ) -> Result<Self, ExecutionError> {
        let key = key.into();
        if key.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("answer key"));
        }
        let own_words = match own_words {
            Some(words) if words.trim().is_empty() => None,
            Some(words) if words.len() > MAX_TEXT_BYTES => {
                return Err(ExecutionError::ValueTooLong {
                    field: "answer text",
                    max_bytes: MAX_TEXT_BYTES,
                })
            }
            Some(words) => Some(words.into_boxed_str()),
            None => None,
        };
        Ok(Self {
            key: key.into_boxed_str(),
            values: values.into_iter().map(String::into_boxed_str).collect(),
            own_words,
        })
    }

    /// The question this answers.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The option values chosen, in the order they were chosen.
    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.values.iter().map(Box::as_ref)
    }

    /// Words of the answerer's own, where they typed any.
    pub fn own_words(&self) -> Option<&str> {
        self.own_words.as_deref()
    }
}

/// An answer that belongs to the ask it answers.
///
/// Construction is the check: every key names a question that was asked, every
/// value was offered by that question, a single-answer question is not given
/// several, and prose only goes where the question invited it. An agent acts on
/// what comes back, so what comes back is what it asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedAnswer(Vec<QuestionChoice>);

impl AcceptedAnswer {
    /// Check `choices` against `question`, keeping them in the asked order.
    pub fn new(
        question: &AgentQuestion,
        choices: Vec<QuestionChoice>,
    ) -> Result<Self, ExecutionError> {
        let mut answered = std::collections::HashSet::with_capacity(choices.len());
        for choice in &choices {
            if !answered.insert(choice.key()) {
                return Err(ExecutionError::DuplicateQuestionKey);
            }
            let asked = question
                .questions()
                .iter()
                .find(|asked| asked.key() == choice.key())
                .ok_or(ExecutionError::UnaskedQuestion)?;
            if choice.values.len() > 1 && asked.shape() == AnswerShape::One {
                return Err(ExecutionError::TooManyValues {
                    field: "answers to one question",
                    max: 1,
                });
            }
            for value in choice.values() {
                if !asked.options().iter().any(|option| option.value() == value) {
                    return Err(ExecutionError::UnofferedAnswer);
                }
            }
            if choice.own_words().is_some() && !asked.free_text() {
                return Err(ExecutionError::UnofferedAnswer);
            }
        }
        Ok(Self(choices))
    }

    /// What was chosen, for the questions that were answered at all.
    pub fn choices(&self) -> &[QuestionChoice] {
        &self.0
    }
}

/// How an ask was answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionResponse {
    /// Answered, with choices that belong to the ask.
    Answered(AcceptedAnswer),
    /// Answered by declining to answer. The agent is told, and continues.
    Declined,
}
