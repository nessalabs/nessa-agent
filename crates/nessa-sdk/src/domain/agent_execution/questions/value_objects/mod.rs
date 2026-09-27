//! What an agent asked and what it will accept, validated at construction.
//!
//! ```text
//! AnswerOption --> Question --> AgentQuestion
//! ```
//!
//! Arrows mean containment: options make up a question, questions make up one
//! ask. Every text is bounded here, because an ask is held for as long as a
//! turn waits for somebody to answer it.
mod answer;
mod identity;
mod question;
mod refusal;
pub use answer::{AcceptedAnswer, QuestionCancellation, QuestionChoice, QuestionResponse};
pub use identity::QuestionId;
pub use question::{
    AgentQuestion, AnswerOption, AnswerShape, Question, MAX_KEY_BYTES, MAX_OPEN_ASK_COST,
    MAX_OPEN_QUESTIONS, MAX_OPTIONS, MAX_QUESTIONS, MAX_TEXT_BYTES,
};
pub use refusal::QuestionRefusalReason;
