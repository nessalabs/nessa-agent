//! Save a typed send, ask the gateway, and read the receipt before any later send.
//!
//! ```text
//! PhoneOutbox --> IntentStore
//!      |              |
//!      +-------> PhoneGateway (conversation.send, conversation.receipt)
//! ```
//!
//! Arrows are calls. The store is written before the gateway is asked.

mod outbox;
mod ports;

pub use outbox::PhoneOutbox;
pub use ports::{CommandIds, IntentStore, OutboxError, PhoneGateway, StoreError, UuidIds};
