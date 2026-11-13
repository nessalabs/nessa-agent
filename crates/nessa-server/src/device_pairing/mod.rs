//! The adapter that meets the conversation context for native device
//! enrollment. Enrollment itself, its listener and the tunnel are the
//! `nessa_device_pairing` crate.
pub mod conversation;
pub use conversation::ConversationReceivers;
