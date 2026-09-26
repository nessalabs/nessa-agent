//! What an agent asked, and what it will accept as an answer.
use crate::domain::agent_execution::ExecutionError;

/// The most questions one ask may carry.
///
/// An agent asking more than this at once is not asking, it is interrogating;
/// the limit also bounds what a host must render and what this retains while a
/// turn waits for an answer.
pub const MAX_QUESTIONS: usize = 16;
/// The most options one question may offer.
pub const MAX_OPTIONS: usize = 32;
/// The most asks that may be open at once.
///
/// One number, because an ask nobody can see is an ask nobody can answer:
/// admitting more than the surface showing them holds would strand the extras
/// with no path to an answer. The binding that admits them and the view that
/// shows them read it from here.
pub const MAX_OPEN_QUESTIONS: usize = 8;
/// The longest prompt, header, option label or description this retains.
pub const MAX_TEXT_BYTES: usize = 1024;
/// The longest field key this retains.
///
/// A key is an identity a host echoes back, and every layer that carries one —
/// the product protocol and the client that validates it — bounds it here. One
/// published contract: a key this accepts and the panel cannot display would be
/// a question nobody could answer.
pub const MAX_KEY_BYTES: usize = 256;

/// How an answer to one question is shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerShape {
    /// Exactly one of the offered options.
    One,
    /// Any number of the offered options, including none.
    Many,
}

/// One thing an agent may be answered with.
///
/// `value` is what the answer records; `label` is what a person reads. They are
/// usually the same text and are kept apart anyway, because the day they differ
/// is the day a host would otherwise send back a label the agent cannot match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnswerOption {
    value: Box<str>,
    label: Box<str>,
    description: Option<Box<str>>,
}

impl AnswerOption {
    /// Offer `value`, shown as `label`, with optional secondary `description`.
    ///
    /// Every string is bounded at [`MAX_TEXT_BYTES`]; an empty value or label is
    /// refused, because neither can be rendered or matched.
    pub fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: Option<String>,
    ) -> Result<Self, ExecutionError> {
        let value = text(value.into(), "answer option value")?;
        let label = text(label.into(), "answer option label")?;
        let description = description
            .map(|value| text(value, "answer option description"))
            .transpose()?;
        Ok(Self {
            value,
            label,
            description,
        })
    }

    /// What an answer records when this option is chosen.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// What a person reads.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Secondary text, where the agent supplied any.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Owned bytes this option retains.
    pub fn payload_bytes(&self) -> usize {
        self.value
            .len()
            .saturating_add(self.label.len())
            .saturating_add(self.description.as_ref().map_or(0, |text| text.len()))
    }
}

/// One question, its offered answers, and whether prose is accepted instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    key: Box<str>,
    prompt: Box<str>,
    header: Option<Box<str>>,
    shape: AnswerShape,
    options: Vec<AnswerOption>,
    free_text_key: Option<Box<str>>,
    required: bool,
}

impl Question {
    /// Ask `prompt` under `key`, answerable by `options` and, where
    /// `free_text_key` names a field, by words of the answerer's own.
    ///
    /// `key` is how the answer is correlated back, so it is required and
    /// bounded like every other identity that crosses these layers.
    /// `free_text_key` is the field the asker named for prose, kept as the
    /// asker wrote it: assuming a name would send the answer to a field the
    /// schema does not have. `required` is the asker saying this one may not be
    /// skipped, which is checked when an answer is accepted rather than
    /// remembered as a comment.
    ///
    /// At least one option is required: a question with nothing to choose and
    /// no free text cannot be answered at all, and an agent that sends one has
    /// asked nothing. Two options that record the same value are refused — a
    /// host choosing between them could not say which it meant.
    pub fn new(
        key: impl Into<String>,
        prompt: impl Into<String>,
        header: Option<String>,
        shape: AnswerShape,
        options: Vec<AnswerOption>,
        free_text_key: Option<String>,
        required: bool,
    ) -> Result<Self, ExecutionError> {
        let key = identity(key.into(), "question key")?;
        let prompt = text(prompt.into(), "question prompt")?;
        let header = header
            .map(|value| text(value, "question header"))
            .transpose()?;
        let free_text_key = free_text_key
            .map(|value| identity(value, "question free-text key"))
            .transpose()?;
        if options.is_empty() {
            return Err(ExecutionError::EmptyValue("question options"));
        }
        if options.len() > MAX_OPTIONS {
            return Err(ExecutionError::TooManyValues {
                field: "question options",
                max: MAX_OPTIONS,
            });
        }
        let mut values = std::collections::HashSet::with_capacity(options.len());
        for option in &options {
            if !values.insert(option.value()) {
                return Err(ExecutionError::DuplicateAnswerOption);
            }
        }
        Ok(Self {
            key,
            prompt,
            header,
            shape,
            options,
            free_text_key,
            required,
        })
    }

    /// How the answer is correlated back to the agent's own field.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// What is being asked.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// A short label for the question, where the agent supplied one.
    pub fn header(&self) -> Option<&str> {
        self.header.as_deref()
    }

    /// Whether one option may be chosen, or several.
    pub fn shape(&self) -> AnswerShape {
        self.shape
    }

    /// What may be chosen.
    pub fn options(&self) -> &[AnswerOption] {
        &self.options
    }

    /// Whether an answer in the answerer's own words is accepted here.
    pub fn free_text(&self) -> bool {
        self.free_text_key.is_some()
    }

    /// The field the asker named for prose, where it invited any.
    pub fn free_text_key(&self) -> Option<&str> {
        self.free_text_key.as_deref()
    }

    /// Whether the asker said this question may not be skipped.
    pub fn required(&self) -> bool {
        self.required
    }

    /// Owned bytes this question retains.
    pub fn payload_bytes(&self) -> usize {
        self.options.iter().fold(
            self.key
                .len()
                .saturating_add(self.prompt.len())
                .saturating_add(self.header.as_ref().map_or(0, |header| header.len()))
                .saturating_add(self.free_text_key.as_ref().map_or(0, |key| key.len())),
            |total, option| total.saturating_add(option.payload_bytes()),
        )
    }
}

/// Everything one ask puts to a person: its framing and its questions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentQuestion {
    message: Box<str>,
    questions: Vec<Question>,
}

impl AgentQuestion {
    /// Ask `questions`, framed by `message`.
    ///
    /// An ask with no questions is refused: there is nothing to put to anybody,
    /// and admitting one would leave a turn waiting on an answer that cannot be
    /// given.
    pub fn new(
        message: impl Into<String>,
        questions: Vec<Question>,
    ) -> Result<Self, ExecutionError> {
        let message = text(message.into(), "question message")?;
        if questions.is_empty() {
            return Err(ExecutionError::EmptyValue("questions"));
        }
        if questions.len() > MAX_QUESTIONS {
            return Err(ExecutionError::TooManyValues {
                field: "questions",
                max: MAX_QUESTIONS,
            });
        }
        let mut keys = std::collections::HashSet::with_capacity(questions.len());
        for question in &questions {
            if !keys.insert(question.key()) {
                return Err(ExecutionError::DuplicateQuestionKey);
            }
        }
        Ok(Self { message, questions })
    }

    /// The agent's own framing of why it is asking.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// What is being asked, in the order the agent asked it.
    pub fn questions(&self) -> &[Question] {
        &self.questions
    }

    /// Owned bytes this ask retains, for the budget that holds it while a turn
    /// waits. Spare capacity is discarded at construction, so this is the text.
    pub fn payload_bytes(&self) -> usize {
        self.questions
            .iter()
            .fold(self.message.len(), |total, question| {
                total.saturating_add(question.payload_bytes())
            })
    }
}

/// Bound and validate one retained identity, which every layer bounds alike.
fn identity(value: String, field: &'static str) -> Result<Box<str>, ExecutionError> {
    if value.trim().is_empty() {
        return Err(ExecutionError::EmptyValue(field));
    }
    if value.len() > MAX_KEY_BYTES {
        return Err(ExecutionError::ValueTooLong {
            field,
            max_bytes: MAX_KEY_BYTES,
        });
    }
    Ok(value.into_boxed_str())
}

/// Bound and validate one piece of retained text.
fn text(value: String, field: &'static str) -> Result<Box<str>, ExecutionError> {
    if value.trim().is_empty() {
        return Err(ExecutionError::EmptyValue(field));
    }
    if value.len() > MAX_TEXT_BYTES {
        return Err(ExecutionError::ValueTooLong {
            field,
            max_bytes: MAX_TEXT_BYTES,
        });
    }
    Ok(value.into_boxed_str())
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/agent_questions.rs"]
mod tests;
