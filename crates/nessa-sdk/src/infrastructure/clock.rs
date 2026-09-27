//! The clock an agent binding measures its protocol deadlines on.
//!
//! Every wait on an ACP agent — `initialize`, session setup, an execution's
//! `execution_timeout`, a steering acknowledgement, cooperative cancellation
//! within `shutdown_grace`, an audit record, reading a message's images, and
//! writing a frame — ends at a [`ClockInstant`] read from this clock, and
//! expires once the clock says it has passed. Composition supplies
//! [`RuntimeClock`] through `AcpConfig::clock`; a test supplies a clock that
//! moves only when the test moves it, so what a deadline passing means is
//! tested without waiting for one, and nothing else a test checks can be cut
//! short by a slow machine
//! (`a_refused_delete_the_list_cannot_explain_stays_the_refusal`).
//!
//! The process's own stop budgets — waiting for it to exit, then
//! `kill_timeout` — are not measured here: they wait on the operating system,
//! not the agent.
#![deny(missing_docs)]

use std::{future::Future, ops::Add, pin::Pin, time::Duration};

/// A moment on one [`Clock`]'s timeline: how long after that clock's origin.
/// Moments from different clocks are not comparable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockInstant(Duration);
impl ClockInstant {
    /// The moment `elapsed` after the clock's origin.
    pub const fn from_origin(elapsed: Duration) -> Self {
        Self(elapsed)
    }
    /// How long after the clock's origin this moment is.
    pub const fn since_origin(self) -> Duration {
        self.0
    }
    /// How long after `earlier` this moment is; zero when it is not after it.
    pub fn saturating_duration_since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
}
/// Saturates rather than overflowing: a moment past the end of time is one no
/// clock reaches.
impl Add<Duration> for ClockInstant {
    type Output = Self;
    fn add(self, after: Duration) -> Self {
        Self(self.0.saturating_add(after))
    }
}

/// Resolves once the clock has reached the moment it was asked to wait for.
pub type ClockSleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Where a binding's protocol deadlines are measured.
pub trait Clock: Send + Sync {
    /// The current moment. Never earlier than one this clock returned before.
    fn now(&self) -> ClockInstant;
    /// Resolves once [`Self::now`] has reached `deadline`, at once if it
    /// already has. It is not polled again after it resolves.
    fn sleep_until(&self, deadline: ClockInstant) -> ClockSleep;
}

/// The async runtime's own clock, measured from when this value was made.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeClock {
    origin: tokio::time::Instant,
}
impl RuntimeClock {
    /// A clock whose origin is now.
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}
impl Default for RuntimeClock {
    fn default() -> Self {
        Self::new()
    }
}
impl Clock for RuntimeClock {
    fn now(&self) -> ClockInstant {
        ClockInstant(self.origin.elapsed())
    }
    fn sleep_until(&self, deadline: ClockInstant) -> ClockSleep {
        match self.origin.checked_add(deadline.0) {
            Some(deadline) => Box::pin(tokio::time::sleep_until(deadline)),
            // Past what the runtime can represent: never reached.
            None => Box::pin(std::future::pending()),
        }
    }
}

/// What `deadline` has become, when it passes first; otherwise what
/// `operation` returned. `operation` is polled first, so one finished at the
/// moment the deadline passes is still its answer.
pub(crate) async fn within<T>(
    clock: &dyn Clock,
    deadline: ClockInstant,
    operation: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! { biased;
        finished = operation => Some(finished),
        () = clock.sleep_until(deadline) => None,
    }
}

// Used only by the ACP tests, which run where their fixtures do.
#[cfg(all(test, unix))]
#[path = "../../tests/infrastructure/manual_clock.rs"]
pub(crate) mod manual;
