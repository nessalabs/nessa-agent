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
#[cfg(test)]
pub(crate) use decide::{after_lookup, after_send, Step};
pub(crate) use decide::{
    apply_lookup, apply_retry, apply_send, budget, retryable, submit_outcome, Applied, Budget,
    Evidence,
};
pub use intent::{IntentError, PhoneCommandId, PhoneImage, PhoneIntent};
pub use outcome::{
    Cause, Delivery, Initiator, LookupAnswer, PhoneFailure, PhoneOutcome, ReceiptEffect,
    ReceiptStage, SendAnswer, UnknownCause,
};
pub use record::{IntentEvent, PhoneRecord, PhoneView};
pub(crate) use restore::{history_reaches, replaces, same_prompt, stored_evidence};

#[cfg(test)]
#[path = "../../../tests/outbox/domain.rs"]
mod tests;
