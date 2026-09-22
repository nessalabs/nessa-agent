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
    AcceptedAnswer, AgentQuestion, AnswerOption, AnswerShape, Question, QuestionChoice, QuestionId,
    QuestionResponse, MAX_OPTIONS, MAX_QUESTIONS, MAX_TEXT_BYTES,
};
