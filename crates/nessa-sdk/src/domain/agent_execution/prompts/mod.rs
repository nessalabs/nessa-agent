//! Preserves prompt text and ordered, attributed system instructions. Callers resolve
//! files and templates before supplying text; this feature has no external effects.
//!
//! ```text
//! attributed text --> SystemPromptBuilder --> SystemPrompt
//! ```
//!
//! Arrows mean assembling contributions into validated immutable instructions.
//! A `UserMessage` is the other thing said to an agent: one turn's text, the
//! images it refers to by digest, and the files it points at by path; who
//! wrote it (`MessageSender`: the person, or an MCP App, `McpAppSource`); and
//! the context apps gave the model with it (`AppModelContext`); and the notes
//! left in the conversation that the turn takes (`NoteContext`); and the
//! earlier turns a native session has not seen (`CarriedHistory`).
pub mod builders;
pub mod value_objects;
pub use builders::SystemPromptBuilder;
pub use value_objects::{
    AppModelContext, CarriedHistory, CarriedTurn, ImageReference, LinkedFile, McpAppSource,
    MessageSender, NoteAuthor, NoteContext, PromptContribution, PromptContributionView,
    PromptSource, PromptSourceKind, PromptText, SystemPrompt, UserMessage,
};
