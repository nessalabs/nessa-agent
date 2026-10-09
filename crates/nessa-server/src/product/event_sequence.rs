//! The socket's one event numbering. `session.challenge` is event
//! [`CHALLENGE_EVENT_SEQUENCE`]; every event after it — a watch notice or a
//! subscription frame — takes the next number when the writer takes it, so
//! one socket never repeats a number and numbers follow write order
//! (`events_continue_the_shared_socket_sequence`).

use super::socket::CHALLENGE_EVENT_SEQUENCE;
use std::sync::atomic::{AtomicU64, Ordering};

pub(in crate::product) struct EventSequence(AtomicU64);

impl Default for EventSequence {
    fn default() -> Self {
        Self(AtomicU64::new(CHALLENGE_EVENT_SEQUENCE))
    }
}

impl EventSequence {
    /// The next number; `None` once the numbering is spent.
    #[allow(deprecated, reason = "Rust 1.89 MSRV; try_update requires Rust 1.95")]
    pub fn next(&self) -> Option<u64> {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |last| {
                last.checked_add(1)
            })
            .ok()
            .map(|last| last + 1)
    }
}
