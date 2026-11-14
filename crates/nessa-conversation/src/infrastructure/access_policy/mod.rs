//! The conversation access policy's adapter: who may locate, act on, read,
//! comment in, drive or share a conversation. Cedar decides, inside `cedar/`.
mod cedar;
pub use cedar::CedarConversationAccessPolicy;
