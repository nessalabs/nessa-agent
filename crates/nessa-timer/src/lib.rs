//! One monotonic timer for absolute deadlines.
//!
//! The SDK's protocol clock and the server's environment waits share this
//! implementation. Wall-clock reads stay with the caller that already captured
//! the instant. `within` polls the operation first and does not let a spent
//! cooperative budget hide an expired deadline.
#![deny(missing_docs)]

use std::{future::Future, ops::Add, pin::Pin, time::Duration};

/// A moment on one [`Timer`]'s timeline: how long after that timer's origin.
/// Moments from different timers are not comparable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Moment(Duration);

impl Moment {
    /// The moment `elapsed` after the timer's origin.
    pub const fn from_origin(elapsed: Duration) -> Self {
        Self(elapsed)
    }

    /// How long after the timer's origin this moment is.
    pub const fn since_origin(self) -> Duration {
        self.0
    }

    /// How long after `earlier` this moment is; zero when it is not after it.
    pub fn since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }

    /// Same measurement as [`Self::since`].
    pub fn saturating_duration_since(self, earlier: Self) -> Duration {
        self.since(earlier)
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
pub type Sleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Where a deadline is measured.
pub trait Timer: Send + Sync {
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
pub struct Elapsed;

/// What `operation` returned, or [`Elapsed`] when `deadline` passes first.
///
/// `operation` is polled first, so one finished at the moment the deadline
/// passes is still its answer. The deadline is polled without the task's
/// cooperative budget, so an operation that is always ready still cannot
/// outlast it.
pub async fn within<T>(
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
pub fn timeout<'a, T: 'a>(
    timer: &'a dyn Timer,
    period: Duration,
    operation: impl Future<Output = T> + 'a,
) -> impl Future<Output = Result<T, Elapsed>> + 'a {
    within(timer, timer.now() + period, operation)
}

/// Tokio's clock, measured from when this value was made.
#[cfg(feature = "runtime")]
#[derive(Clone, Copy, Debug)]
pub struct TokioTimer {
    origin: tokio::time::Instant,
}

#[cfg(feature = "runtime")]
impl TokioTimer {
    /// A timer whose origin is now.
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }

    /// The runtime instant of `moment` on this timer, when it can be represented.
    pub fn instant_at(&self, moment: Moment) -> Option<tokio::time::Instant> {
        self.origin.checked_add(moment.since_origin())
    }
}

#[cfg(feature = "runtime")]
impl Default for TokioTimer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "runtime")]
impl Timer for TokioTimer {
    fn now(&self) -> Moment {
        Moment::from_origin(self.origin.elapsed())
    }

    fn sleep_until(&self, deadline: Moment) -> Sleep {
        match self.instant_at(deadline) {
            Some(deadline) => Box::pin(tokio::time::sleep_until(deadline)),
            None => Box::pin(std::future::pending()),
        }
    }
}

/// A timer that moves only when a test moves it.
#[cfg(feature = "test-support")]
pub mod manual {
    use super::{Moment, Sleep, Timer};
    use std::{sync::Mutex, time::Duration};
    use tokio::sync::watch;

    /// How long, in real time, a test waits for a wait to start.
    const STARTING: Duration = Duration::from_secs(5);

    /// One wait started on a [`ManualTimer`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Wait {
        /// When it started.
        pub started: Moment,
        /// When it ends.
        pub deadline: Moment,
    }

    impl Wait {
        /// How long it was given.
        pub fn limit(self) -> Duration {
            self.deadline.since(self.started)
        }
    }

    /// A timer at its origin until the test advances it, recording every wait.
    pub struct ManualTimer {
        now: watch::Sender<Moment>,
        waits: Mutex<Vec<Wait>>,
        started: watch::Sender<usize>,
    }

    impl Default for ManualTimer {
        fn default() -> Self {
            Self {
                now: watch::channel(Moment::from_origin(Duration::ZERO)).0,
                waits: Mutex::default(),
                started: watch::channel(0).0,
            }
        }
    }

    impl ManualTimer {
        /// Move the timer `by` forward, ending every wait whose deadline that reaches.
        pub fn advance(&self, by: Duration) {
            self.now.send_modify(|now| *now = *now + by);
        }

        /// Move the timer forward to `moment`; never back.
        pub fn advance_to(&self, moment: Moment) {
            self.now.send_modify(|now| *now = (*now).max(moment));
        }

        /// How long after its origin the timer is.
        pub fn elapsed(&self) -> Duration {
            self.now().since_origin()
        }

        /// Every wait started so far, in order.
        pub fn waits(&self) -> Vec<Wait> {
            self.waits.lock().expect("waits").clone()
        }

        /// The first wait matching `wanted`, once it has started.
        ///
        /// # Panics
        /// None has started within [`STARTING`] of real time.
        pub async fn started(&self, wanted: impl Fn(usize, &Wait) -> bool) -> Wait {
            let mut started = self.started.subscribe();
            let found = async {
                loop {
                    if let Some((_, wait)) = self
                        .waits()
                        .iter()
                        .enumerate()
                        .find(|(index, wait)| wanted(*index, wait))
                    {
                        return *wait;
                    }
                    started.changed().await.expect("wait counter");
                }
            };
            match tokio::time::timeout(STARTING, found).await {
                Ok(wait) => wait,
                Err(_) => panic!("no such wait started; waits: {:?}", self.waits()),
            }
        }

        /// The `index`th wait (from zero), once it has started.
        pub async fn nth(&self, index: usize) -> Wait {
            self.started(|at, _| at == index).await
        }

        /// Once the `index`th wait has started, move the timer to its deadline.
        pub async fn run_out(&self, index: usize) -> Wait {
            let wait = self.nth(index).await;
            self.advance_to(wait.deadline);
            wait
        }

        /// `operation`, with every wait run out as soon as it starts.
        pub async fn passing<T>(&self, operation: impl std::future::Future<Output = T>) -> T {
            let mut started = self.started.subscribe();
            let passing = async {
                loop {
                    if let Some(latest) = self.waits().iter().map(|wait| wait.deadline).max() {
                        self.advance_to(latest);
                    }
                    started.changed().await.expect("wait counter");
                }
            };
            tokio::select! {
                biased;
                answer = operation => answer,
                never = passing => never,
            }
        }
    }

    impl Timer for ManualTimer {
        fn now(&self) -> Moment {
            *self.now.borrow()
        }

        fn sleep_until(&self, deadline: Moment) -> Sleep {
            let mut now = self.now.subscribe();
            self.waits.lock().expect("waits").push(Wait {
                started: *now.borrow(),
                deadline,
            });
            self.started.send_modify(|started| *started += 1);
            Box::pin(async move {
                if now.wait_for(|now| *now >= deadline).await.is_err() {
                    std::future::pending::<()>().await;
                }
            })
        }
    }
}

#[cfg(feature = "test-support")]
pub use manual::ManualTimer;

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use std::future::poll_fn;
    use std::task::Poll;

    fn manual() -> ManualTimer {
        ManualTimer::default()
    }

    #[tokio::test]
    async fn operation_ready_with_the_deadline_wins() {
        let timer = manual();
        let deadline = timer.now();
        let answer = within(&timer, deadline, async { 7 }).await;
        assert_eq!(answer, Ok(7));
    }

    #[tokio::test]
    async fn a_spent_budget_still_observes_the_deadline() {
        let timer = manual();
        let deadline = timer.now();
        let operation = poll_fn(|context| {
            context.waker().wake_by_ref();
            Poll::<()>::Pending
        });
        let answer = tokio::spawn(async move { within(&timer, deadline, operation).await });
        assert_eq!(answer.await.expect("join"), Err(Elapsed));
    }

    #[tokio::test]
    async fn sleep_before_at_and_after_the_deadline_uses_one_instant() {
        let timer = manual();
        let deadline = timer.now() + Duration::from_millis(5);
        let waiting = timer.sleep_until(deadline);
        tokio::pin!(waiting);
        poll_fn(|context| {
            assert_eq!(waiting.as_mut().poll(context), Poll::Pending);
            Poll::Ready(())
        })
        .await;
        timer.advance(Duration::from_millis(5));
        waiting.await;
        assert_eq!(timer.elapsed(), Duration::from_millis(5));
        timer.sleep_until(timer.now()).await;
        let later = timer.now() + Duration::from_millis(1);
        let again = timer.now();
        assert_eq!(later.since(again), Duration::from_millis(1));
    }

    #[tokio::test]
    async fn dropped_wait_releases_and_manual_timers_share_or_isolate_time() {
        let timer = manual();
        let wait = timer.sleep_until(timer.now() + Duration::from_secs(1));
        drop(wait);
        let cloned = &timer;
        cloned.advance(Duration::from_millis(3));
        assert_eq!(timer.elapsed(), Duration::from_millis(3));
        let other = manual();
        assert_eq!(other.elapsed(), Duration::ZERO);
    }
}
