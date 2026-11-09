//! Immutable prompt text, source attribution, contributions, and composed instructions.
//!
//! ```text
//! PromptSource + text --> PromptContribution --> SystemPrompt
//! ```
//!
//! Arrows mean consuming contributions into one text backing with source ranges.
//! PromptContributionView borrows each attributed range without copying text.
//!
//! ```text
//! PromptText + ImageReference(Sha256Digest, ImageMediaType, size)
//!             + LinkedFile(absolute path)                --> UserMessage
//! ```
//!
//! A user message refers to its images by digest and never holds their bytes,
//! and to its files by path, which it never opens. The two are separate because
//! an image's bytes travel with the message and a file's stay where they are.
//!
//! ```text
//! McpAppSource --> MessageSender::App --> UserMessage (who wrote it)
//!     `--------> AppModelContext ------------> UserMessage (what goes with it)
//! ```
//!
//! An MCP App can speak in the person's turn: write the message, on their
//! behalf, or give the model context that goes ahead of a message and is not
//! part of what it says. Arrows mean "is held by".
//!
//! ```text
//! NoteAuthor + surface + text --> NoteContext --> UserMessage (what goes with it)
//! ```
//!
//! A note someone left in the conversation goes with the turn that takes it,
//! ahead of the message and not part of what it says.
//!
//! ```text
//! CarriedTurn(said, reply, agent origin)* --> CarriedHistory --> UserMessage (what goes with it)
//! ```
//!
//! The earlier turns a native session has not seen, after a fork or an agent
//! switch, go with the first turn that runs in it, ahead of the message.
mod prompt;
pub use prompt::{
    PromptContribution, PromptContributionView, PromptSource, PromptSourceKind, PromptText,
    SystemPrompt,
};

mod app_message;
pub use app_message::{AppModelContext, McpAppSource, MessageSender};

mod carried_history;
pub use carried_history::{CarriedHistory, CarriedTurn};

mod note_context;
pub use note_context::{NoteAuthor, NoteContext};

mod user_message;
pub use user_message::{ImageReference, LinkedFile, UserMessage};

mod uploaded_file;
pub use uploaded_file::UploadedFileReference;
