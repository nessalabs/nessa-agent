#![deny(missing_docs)]

use crate::domain::agent_execution::ExecutionError;

/// Under which authority a note was left: the conversation's owner, or
/// someone holding a share grant on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteAuthor {
    /// The conversation's owner left it.
    Owner,
    /// Someone the conversation is shared with left it, under their grant.
    Grant,
    /// An agent in another turn left it.
    Agent,
}

/// One note someone left in the conversation, as the model is given it with
/// the turn that takes it: who left it, from which surface, and what it says.
///
/// It travels with a user message, ahead of what the message says, and is
/// not part of what the person said: the conversation's records keep the
/// note, and the turn's saved message never holds it. Nothing here is
/// authenticated; the session builds it from a note it saved, which the host
/// admitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteContext {
    author: NoteAuthor,
    surface: Box<str>,
    text: Box<str>,
}
impl NoteContext {
    /// Most UTF-8 bytes of a note's text, the bound a note is left under.
    pub const MAX_TEXT_BYTES: usize = 8192;
    /// Most UTF-8 bytes of the surface it was left from, the bound a verified
    /// surface identity has.
    pub const MAX_SURFACE_BYTES: usize = 256;

    /// The note `text` left by `author` from `surface`.
    ///
    /// # Errors
    ///
    /// [`ExecutionError::EmptyValue`] for a blank surface or text;
    /// [`ExecutionError::ValueTooLong`] for a surface past
    /// [`Self::MAX_SURFACE_BYTES`] or a text past [`Self::MAX_TEXT_BYTES`].
    ///
    /// # Examples
    ///
    /// ```
    /// use nessa_sdk::domain::agent_execution::prompts::{NoteAuthor, NoteContext};
    ///
    /// let note = NoteContext::new(NoteAuthor::Grant, "phone", "Use the May figures")?;
    /// assert_eq!(note.author(), NoteAuthor::Grant);
    /// assert_eq!(note.surface(), "phone");
    /// assert_eq!(note.text(), "Use the May figures");
    /// assert!(NoteContext::new(NoteAuthor::Owner, "desktop", " ").is_err());
    /// # Ok::<(), nessa_sdk::domain::agent_execution::ExecutionError>(())
    /// ```
    pub fn new(author: NoteAuthor, surface: &str, text: &str) -> Result<Self, ExecutionError> {
        Ok(Self {
            author,
            surface: bounded(surface, "note surface", Self::MAX_SURFACE_BYTES)?,
            text: bounded(text, "note text", Self::MAX_TEXT_BYTES)?,
        })
    }
    /// Under which authority it was left.
    pub fn author(&self) -> NoteAuthor {
        self.author
    }
    /// The surface it was left from.
    pub fn surface(&self) -> &str {
        &self.surface
    }
    /// What it says, exactly as left.
    pub fn text(&self) -> &str {
        &self.text
    }
}

fn bounded(value: &str, field: &'static str, max_bytes: usize) -> Result<Box<str>, ExecutionError> {
    if value.trim().is_empty() {
        return Err(ExecutionError::EmptyValue(field));
    }
    if value.len() > max_bytes {
        return Err(ExecutionError::ValueTooLong { field, max_bytes });
    }
    Ok(value.into())
}
