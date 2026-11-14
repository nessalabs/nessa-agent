//! Where attachments (the `nessa-attachments` crate) meet the rest of the
//! gateway: the HTTP upload route, which answers with the server's CORS
//! rules, and the adapters that meet the conversation context through its
//! own ports.
pub use nessa_conversation::attachments as conversation;
pub mod http;

pub use conversation::{ConversationHolds, RepositoryOwnership};

#[cfg(test)]
#[path = "../../tests/attachments/adapters.rs"]
mod adapter_tests;
