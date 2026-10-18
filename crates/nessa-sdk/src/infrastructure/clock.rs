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

use std::{future::Future, ops::Add, pin::Pin, sync::OnceLock, time::Duration};

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
    /// Unix milliseconds at `at`, a moment on this clock.
    ///
    /// A permission's `dueAtMs` is this at the moment its deadline is armed,
    /// plus the wait, so a client counts down on the same clock that decides
    /// the review. It converts a moment already read rather than reading the
    /// time again: two reads can straddle a pause, and then the card and the
    /// deadline disagree by that pause. [`RuntimeClock`] converts onto the
    /// process's one wall timeline, [`process_unix_ms`], which a gateway also
    /// stamps its views from. A later read of the wall clock is not a second
    /// opinion.
    fn unix_ms_at(&self, at: ClockInstant) -> u64;
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

/// Unix milliseconds on this process's one wall timeline: the wall clock read
/// once, the first time anything asks, then followed by the monotonic clock.
///
/// Every [`RuntimeClock`] reports it, and a gateway stamps the time it tells a
/// client from it, so a deadline's `dueAtMs` and the gateway's "now" stay one
/// timeline when the operating system later corrects its wall clock. Read
/// separately, a correction made after an agent opened moved one and not the
/// other, and a countdown ended early or ran long by the correction
/// (`a_view_carries_the_gateways_time_from_its_clock` in `nessa-server`).
pub fn process_unix_ms() -> u64 {
    process_unix_ms_at(std::time::Instant::now())
}
/// [`process_unix_ms`] at `instant` rather than now.
fn process_unix_ms_at(instant: std::time::Instant) -> u64 {
    static ANCHOR: OnceLock<(std::time::Instant, u64)> = OnceLock::new();
    let (origin, unix_origin_ms) =
        *ANCHOR.get_or_init(|| (std::time::Instant::now(), unix_epoch_ms()));
    let millis = |span: Duration| u64::try_from(span.as_millis()).unwrap_or(u64::MAX);
    match instant.checked_duration_since(origin) {
        Some(after) => unix_origin_ms.saturating_add(millis(after)),
        None => unix_origin_ms.saturating_sub(millis(origin.duration_since(instant))),
    }
}
fn unix_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|now| u64::try_from(now.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
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
    fn unix_ms_at(&self, at: ClockInstant) -> u64 {
        match self.origin.checked_add(at.0) {
            Some(instant) => process_unix_ms_at(instant.into_std()),
            // Past what the runtime can represent: no wall time reaches it.
            None => u64::MAX,
        }
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
        () = tokio::task::unconstrained(clock.sleep_until(deadline)) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_runtime_clock_reads_the_one_process_timeline() {
        let first = process_unix_ms();
        let earlier = RuntimeClock::new();
        std::thread::sleep(Duration::from_millis(5));
        let later = RuntimeClock::new();
        // Clocks made at different moments still agree on what time it is,
        // and that time is the process timeline's, which only moves forward.
        let (a, b) = (
            earlier.unix_ms_at(earlier.now()),
            later.unix_ms_at(later.now()),
        );
        let last = process_unix_ms();
        assert!(first <= a && a <= b && b <= last, "{first} {a} {b} {last}");
        assert!(last - first < 60_000);
    }
}

// Used only by the ACP tests, which run where their fixtures do.
#[cfg(all(test, unix))]
#[path = "../../tests/infrastructure/manual_clock.rs"]
pub(crate) mod manual;
