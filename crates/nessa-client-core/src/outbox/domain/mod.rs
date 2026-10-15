//! Immutable send identity, canonical bytes, and the outcome transitions.
//!
//! ```text
//! typed text + ids --> PhoneIntent (canonical bytes)
//! gateway answer --> PhoneRecord --> PhoneOutcome
//! ```
//!
//! Arrows are pure values. Nothing here opens a store or a socket.

mod bytes;
mod decide;
mod intent;
mod outcome;
mod record;
mod restore;

pub use decide::FOREGROUND_SENDS;
pub(crate) use decide::{after_lookup, after_send, submit_outcome, Step};
pub use intent::{IntentError, PhoneCommandId, PhoneImage, PhoneIntent};
pub use outcome::{
    Cause, Delivery, Initiator, LookupAnswer, PhoneFailure, PhoneOutcome, ReceiptEffect,
    ReceiptStage, SendAnswer, UnknownCause,
};
pub use record::{IntentEvent, PhoneRecord, PhoneView};
pub(crate) use restore::{history_reaches, replaces, same_prompt, stored_evidence, Evidence};

#[cfg(test)]
#[path = "../../../tests/outbox/domain.rs"]
mod tests;
