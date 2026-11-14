//! A conversation owns its organization and principal association, and once
//! deleted, the tombstone the access policy refuses callers by.
mod conversation;
pub use conversation::{Conversation, ConversationRefusal};
