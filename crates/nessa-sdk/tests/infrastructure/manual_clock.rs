//! A [`Clock`] that moves only when a test moves it.
//!
//! Waiting is the shared manual timer. This type only adds the wall-time
//! mapping a permission deadline needs.
use super::{Clock, ClockInstant, ClockSleep};
use nessa_timer::{ManualTimer, Moment, Timer};
use std::time::Duration;

/// One wait started on a [`ManualClock`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Wait {
    /// When it started.
    pub started: ClockInstant,
    /// When it ends.
    pub deadline: ClockInstant,
}
impl Wait {
    /// How long it was given.
    pub fn limit(self) -> Duration {
        self.deadline.saturating_duration_since(self.started)
    }
}

pub(crate) struct ManualClock {
    timer: ManualTimer,
    /// Unix milliseconds at the origin. A test's `dueAtMs` is this plus
    /// [`Clock::now`], so it cannot be the process wall clock.
    unix_origin_ms: u64,
}
impl Default for ManualClock {
    fn default() -> Self {
        Self {
            timer: ManualTimer::default(),
            unix_origin_ms: 0,
        }
    }
}
impl ManualClock {
    fn mapped(wait: &nessa_timer::manual::Wait) -> Wait {
        Wait {
            started: ClockInstant::from_origin(wait.started.since_origin()),
            deadline: ClockInstant::from_origin(wait.deadline.since_origin()),
        }
    }
    /// A clock whose wall time starts at `unix_origin_ms`.
    pub fn at_unix_ms(unix_origin_ms: u64) -> Self {
        Self {
            unix_origin_ms,
            ..Self::default()
        }
    }
    /// Move the clock `by` forward, ending every wait whose deadline that
    /// reaches.
    pub fn advance(&self, by: Duration) {
        self.timer.advance(by);
    }
    /// Move the clock forward to `moment`; never back.
    pub fn advance_to(&self, moment: ClockInstant) {
        self.timer
            .advance_to(Moment::from_origin(moment.since_origin()));
    }
    /// Every wait started so far, in order.
    pub fn waits(&self) -> Vec<Wait> {
        self.timer.waits().iter().map(Self::mapped).collect()
    }
    /// Once a wait matching `wanted` has started, move the clock to its
    /// deadline, and answer that wait.
    pub async fn run_out(&self, wanted: impl Fn(&Wait) -> bool) -> Wait {
        let wait = self
            .timer
            .started(|_, wait| wanted(&Self::mapped(wait)))
            .await;
        self.timer.advance_to(wait.deadline);
        Self::mapped(&wait)
    }
    /// `operation`, with every wait matching `passes` run out as soon as it
    /// starts: time passes for the waits a test is not about — a stop's
    /// grace, say — and no others.
    pub async fn passing<T>(
        &self,
        passes: impl Fn(&Wait) -> bool,
        operation: impl std::future::Future<Output = T>,
    ) -> T {
        let timer = &self.timer;
        let passing = async {
            let mut seen = 0usize;
            loop {
                let waits = timer.waits();
                if waits.len() != seen {
                    seen = waits.len();
                }
                if let Some(latest) = waits
                    .iter()
                    .filter(|wait| passes(&Self::mapped(wait)))
                    .map(|wait| wait.deadline)
                    .max()
                {
                    timer.advance_to(latest);
                }
                if timer.waits().len() == seen {
                    timer.started(|index, _| index + 1 > seen).await;
                }
            }
        };
        tokio::select! { biased;
            answer = operation => answer,
            never = passing => never,
        }
    }
}
impl Clock for ManualClock {
    fn now(&self) -> ClockInstant {
        ClockInstant::from_origin(self.timer.now().since_origin())
    }
    fn unix_ms_at(&self, at: ClockInstant) -> u64 {
        let elapsed = u64::try_from(at.since_origin().as_millis()).unwrap_or(u64::MAX);
        self.unix_origin_ms.saturating_add(elapsed)
    }
    fn sleep_until(&self, deadline: ClockInstant) -> ClockSleep {
        self.timer
            .sleep_until(Moment::from_origin(deadline.since_origin()))
    }
}
