//! A [`Clock`] that moves only when a test moves it.
//!
//! A deadline started on it passes only when the test advances the clock to
//! it, so a test says exactly when the agent's time runs out, and nothing it
//! checks can be cut short by a slow machine. Waiting on a real process is
//! still bounded in real time by the test itself.
use super::{Clock, ClockInstant, ClockSleep};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{sync::Mutex, time::Duration};
use tokio::sync::watch;

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
    now: watch::Sender<ClockInstant>,
    /// Every wait started, in order.
    waits: Mutex<Vec<Wait>>,
    started: watch::Sender<usize>,
    /// When set, a sleep whose deadline has already arrived waits until
    /// [`Self::hold_already_due`] is cleared. The HTTP writer uses that sleep
    /// after it settles, so a test can let recovery publish first.
    hold_due: AtomicBool,
    hold_released: watch::Sender<u64>,
}
impl Default for ManualClock {
    fn default() -> Self {
        Self {
            now: watch::channel(ClockInstant::from_origin(Duration::ZERO)).0,
            waits: Mutex::default(),
            started: watch::channel(0).0,
            hold_due: AtomicBool::new(false),
            hold_released: watch::channel(0).0,
        }
    }
}
impl ManualClock {
    /// Hold or release sleeps that are already due. The writer's post-settlement
    /// sleep is one of those. Other deadlines keep their own time.
    pub fn hold_already_due(&self, hold: bool) {
        self.hold_due.store(hold, Ordering::SeqCst);
        if !hold {
            self.hold_released
                .send_modify(|generation| *generation = generation.wrapping_add(1));
        }
    }
    /// Move the clock `by` forward, ending every wait whose deadline that
    /// reaches.
    pub fn advance(&self, by: Duration) {
        self.now.send_modify(|now| *now = *now + by);
    }
    /// Move the clock forward to `moment`; never back.
    pub fn advance_to(&self, moment: ClockInstant) {
        self.now.send_modify(|now| *now = (*now).max(moment));
    }
    /// Every wait started so far, in order.
    pub fn waits(&self) -> Vec<Wait> {
        self.waits.lock().unwrap().clone()
    }
    /// Once a wait matching `wanted` has started, move the clock to its
    /// deadline, and answer that wait.
    pub async fn run_out(&self, wanted: impl Fn(&Wait) -> bool) -> Wait {
        let mut started = self.started.subscribe();
        loop {
            if let Some(wait) = self.waits().into_iter().find(|wait| wanted(wait)) {
                self.advance_to(wait.deadline);
                return wait;
            }
            started.changed().await.unwrap();
        }
    }
    /// `operation`, with every wait matching `passes` run out as soon as it
    /// starts: time passes for the waits a test is not about — a stop's
    /// grace, say — and no others.
    pub async fn passing<T>(
        &self,
        passes: impl Fn(&Wait) -> bool,
        operation: impl std::future::Future<Output = T>,
    ) -> T {
        let mut started = self.started.subscribe();
        let passing = async {
            loop {
                let waits = self.waits();
                if let Some(latest) = waits
                    .iter()
                    .filter(|wait| passes(wait))
                    .map(|wait| wait.deadline)
                    .max()
                {
                    self.advance_to(latest);
                }
                started.changed().await.unwrap();
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
        *self.now.borrow()
    }
    fn sleep_until(&self, deadline: ClockInstant) -> ClockSleep {
        let mut now = self.now.subscribe();
        let started_at = *now.borrow();
        self.waits.lock().unwrap().push(Wait {
            started: started_at,
            deadline,
        });
        self.started.send_modify(|started| *started += 1);
        if self.hold_due.load(Ordering::SeqCst) && deadline <= started_at {
            let mut released = self.hold_released.subscribe();
            let seen = *released.borrow_and_update();
            return Box::pin(async move {
                if released
                    .wait_for(|generation| *generation != seen)
                    .await
                    .is_err()
                {
                    std::future::pending::<()>().await;
                }
            });
        }
        Box::pin(async move {
            // A clock nobody holds any more never moves again.
            if now.wait_for(|now| *now >= deadline).await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }
}
