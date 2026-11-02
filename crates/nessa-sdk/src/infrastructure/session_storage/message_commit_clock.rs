//! Tokio adapter for the application-owned streaming message commit clock.
#![deny(missing_docs)]

use std::time::Duration;

use crate::application::agent_execution::sessions::{MessageCommitClock, MessageCommitSleep};

/// Monotonic Tokio time for one or more session managers, and the process
/// wall timeline.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeMessageCommitClock {
    origin: tokio::time::Instant,
}
impl RuntimeMessageCommitClock {
    /// Starts one clock at the current Tokio instant.
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}
impl Default for RuntimeMessageCommitClock {
    fn default() -> Self {
        Self::new()
    }
}
impl MessageCommitClock for RuntimeMessageCommitClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
    fn sleep_until(&self, deadline: Duration) -> MessageCommitSleep {
        match self.origin.checked_add(deadline) {
            Some(instant) => Box::pin(tokio::time::sleep_until(instant)),
            None => Box::pin(std::future::pending()),
        }
    }
    /// Unix milliseconds on the process wall timeline
    /// ([`crate::infrastructure::clock::process_unix_ms`]). One before the
    /// epoch reads as the epoch. The queue that feeds a session stamps from
    /// this same clock, so the runtime timeline stays here and a substituted
    /// clock replaces both.
    fn unix_milliseconds(&self) -> u64 {
        crate::infrastructure::clock::process_unix_ms()
    }
}
