//! Monotonic wake source for session saves and scheduling audit observations, and
//! the wall clock a conversation dates what it accepts by.
#![deny(missing_docs)]

use std::{future::Future, pin::Pin, time::Duration};

/// A wake that resolves after one deadline on its clock's timeline.
pub type MessageCommitSleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Monotonic time and deadline wakes for session saves and scheduling audits,
/// and the wall-clock time a conversation dates a note by.
///
/// Its `Duration` values measure time since that clock's origin. A manager
/// compares only values returned by its injected clock; hosts may share one
/// clock among managers. The wall clock is separate: it may be set back or
/// forward, so nothing waits on it, and a rule that reads it must hold
/// whichever way it moved.
pub trait MessageCommitClock: Send + Sync {
    /// Returns the current elapsed time since this clock's origin.
    fn now(&self) -> Duration;
    /// Resolves when `deadline` is reached, including when it has passed.
    fn sleep_until(&self, deadline: Duration) -> MessageCommitSleep;
    /// The current wall-clock time, in milliseconds since the Unix epoch.
    fn unix_milliseconds(&self) -> u64;
}
