//! Tokio adapter for the application-owned streaming message commit clock.
#![deny(missing_docs)]

use std::time::Duration;

use nessa_timer::Timer;

use crate::application::agent_execution::sessions::{MessageCommitClock, MessageCommitSleep};

/// Monotonic Tokio time for one or more session managers, and the system's
/// wall clock.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeMessageCommitClock {
    timer: nessa_timer::TokioTimer,
}
impl RuntimeMessageCommitClock {
    /// Starts one clock at the current Tokio instant.
    pub fn new() -> Self {
        Self {
            timer: nessa_timer::TokioTimer::new(),
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
        self.timer.now().since_origin()
    }
    fn sleep_until(&self, deadline: Duration) -> MessageCommitSleep {
        self.timer
            .sleep_until(nessa_timer::Moment::from_origin(deadline))
    }
    /// The system clock; one before the epoch reads as the epoch.
    fn unix_milliseconds(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }
}
