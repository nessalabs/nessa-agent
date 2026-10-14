//! The phone's durable outbox for one typed send.
//!
//! ```text
//! hold --> private store (draft)
//! send --> store departed --> conversation.send
//!              |                    |
//!              |              lost answer --> conversation.receipt
//!              |                    |
//!         reopen / recover ---------+--> rendered outcome
//! ```
//!
//! Arrows are calls. The store write that marks a send as departed finishes
//! before `conversation.send` is asked (`departure_is_durable_before_the_gateway_is_asked`).
//! A draft is not a submission (`a_saved_draft_is_not_a_submission`).
//! The [phone send contract](../../../docs/design/phone-send-contract.md) owns
//! the wire fields and the five outcomes. The ordering table is
//! [phone outbox](../../../docs/design/phone-outbox.md).
//! Cedar admits the session that carries the two calls. This module does not.

mod application;
mod domain;
mod infrastructure;

pub use application::{CommandIds, IntentStore, OutboxError, PhoneGateway, PhoneOutbox, UuidIds};
pub use domain::{
    Cause, Delivery, Initiator, IntentError, LookupAnswer, PhoneCommandId, PhoneFailure,
    PhoneImage, PhoneIntent, PhoneOutcome, PhoneRecord, PhoneView, ReceiptEffect, ReceiptStage,
    SendAnswer, UnknownCause, FOREGROUND_SENDS,
};
pub use infrastructure::open;
pub(crate) use infrastructure::{
    command_failure, lookup_answer, receipt_params, send_answer, send_params,
};
