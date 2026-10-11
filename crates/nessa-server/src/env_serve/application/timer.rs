//! The timer the environment role waits on (issue #758): every deadline,
//! interval and wait in `serve.rs`, `redial.rs` and `approval.rs` is
//! measured on a [`Timer`] they are given, never on the runtime's clock.
//!
//! ```text
//! composition ──▶ TokioTimer (infrastructure) ──▶ serve, redial, approval
//! a test      ──▶ ManualTimer                 ──▶ the same, moved by the test
//! ```
//!
//! Arrows are what each is given. A test says exactly when a wait ends, so
//! what a deadline passing means is tested without waiting for one, and a
//! slow machine cannot cut a wait short. What the host's own ports take as
//! a budget (a harness's cleanup, a command's timeout) is theirs to measure.
use std::{future::Future, ops::Add, pin::Pin, time::Duration};

/// A moment on one [`Timer`]'s timeline: how long after that timer's origin.
/// Moments from different timers are not comparable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Moment(Duration);

impl Moment {
    /// The moment `elapsed` after the timer's origin.
    pub(crate) const fn from_origin(elapsed: Duration) -> Self {
        Self(elapsed)
    }
    /// How long after the timer's origin this moment is.
    pub(crate) const fn since_origin(self) -> Duration {
        self.0
    }
    /// How long after `earlier` this moment is; zero when it is not after it.
    pub(crate) fn since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
}

/// Saturates rather than overflowing: a moment past the end of time is one
/// no timer reaches.
impl Add<Duration> for Moment {
    type Output = Self;
    fn add(self, after: Duration) -> Self {
        Self(self.0.saturating_add(after))
    }
}

/// Resolves once the timer has reached the moment it was asked to wait for.
pub(crate) type Sleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Where the environment role's waits are measured.
pub(crate) trait Timer: Send + Sync {
    /// The current moment. Never earlier than one this timer returned before.
    fn now(&self) -> Moment;
    /// Resolves once [`Self::now`] has reached `deadline`, at once if it
    /// already has. It is not polled again after it resolves.
    fn sleep_until(&self, deadline: Moment) -> Sleep;
    /// Resolves once `period` has passed from now.
    fn sleep(&self, period: Duration) -> Sleep {
        self.sleep_until(self.now() + period)
    }
}

/// A deadline passed before the operation it bounded finished.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Elapsed;

/// What `operation` returned, or [`Elapsed`] when `deadline` passes first.
/// `operation` is polled first, so one finished at the moment the deadline
/// passes is still its answer. As in Tokio's own `timeout`, the deadline is
/// seen even when `operation` has spent the task's budget: an operation
/// always ready to make progress still cannot outlast it.
pub(crate) async fn within<T>(
    timer: &dyn Timer,
    deadline: Moment,
    operation: impl Future<Output = T>,
) -> Result<T, Elapsed> {
    tokio::select! {
        biased;
        finished = operation => Ok(finished),
        () = tokio::task::unconstrained(timer.sleep_until(deadline)) => Err(Elapsed),
    }
}

/// [`within`] a deadline `period` from now, read when this is called.
pub(crate) fn timeout<'a, T: 'a>(
    timer: &'a dyn Timer,
    period: Duration,
    operation: impl Future<Output = T> + 'a,
) -> impl Future<Output = Result<T, Elapsed>> + 'a {
    within(timer, timer.now() + period, operation)
}

#[cfg(test)]
#[path = "../../../tests/env_serve/manual_timer.rs"]
pub(crate) mod manual;
