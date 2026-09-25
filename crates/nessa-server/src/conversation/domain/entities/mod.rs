//! A conversation owns its organization and principal association, and once
//! deleted, the tombstone `check_access` refuses callers by.
mod conversation;
pub use conversation::{Conversation, ConversationRefusal};
