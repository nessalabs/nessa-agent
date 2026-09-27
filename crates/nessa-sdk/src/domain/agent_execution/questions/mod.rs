//! An agent's own question to a person, and the answer it is given.
//!
//! ```text
//! AgentQuestion --> QuestionAnswer
//! ```
//!
//! Arrows mean an ask is resolved by exactly one answer. A question is not a
//! permission: nothing is being authorised, and declining one is an answer
//! rather than a refusal.
pub mod value_objects;
pub use value_objects::{
    AcceptedAnswer, AgentQuestion, AnswerOption, AnswerShape, Question, QuestionCancellation,
    QuestionChoice, QuestionId, QuestionRefusalReason, QuestionResponse, MAX_KEY_BYTES,
    MAX_OPEN_ASK_COST, MAX_OPEN_QUESTIONS, MAX_OPTIONS, MAX_QUESTIONS, MAX_TEXT_BYTES,
};
