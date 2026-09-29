//! Monotonic wake source for the session's streaming-message save owner.
#![deny(missing_docs)]

use std::{future::Future, pin::Pin, time::Duration};

/// A wake that resolves after one deadline on its clock's timeline.
pub type MessageCommitSleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Monotonic time and deadline wakes required by streaming message commits.
///
/// Its `Duration` values measure time since that clock's origin. A manager
/// compares only values returned by its injected clock; hosts may share one
/// clock among managers.
pub trait MessageCommitClock: Send + Sync {
    /// Returns the current elapsed time since this clock's origin.
    fn now(&self) -> Duration;
    /// Resolves when `deadline` is reached, including when it has passed.
    fn sleep_until(&self, deadline: Duration) -> MessageCommitSleep;
}
