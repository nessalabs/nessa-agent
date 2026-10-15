//! The private SQLite outbox and the product-call encoding.
//!
//! ```text
//! PhoneOutbox::open --> schema.sql
//! send params ------> conversation.send / conversation.receipt
//! ```
//!
//! Arrows are calls. The session adapter asks those two methods and does not
//! admit a command on its own.

mod sqlite;
mod wire;

pub use sqlite::open;
pub use wire::{command_failure, lookup_answer, receipt_params, send_answer, send_params};

#[cfg(test)]
#[path = "../../../tests/outbox/store.rs"]
mod tests;
