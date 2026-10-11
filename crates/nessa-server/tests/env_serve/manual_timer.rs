//! A [`Timer`] that moves only when a test moves it.
//!
//! A wait started on it ends only when the test advances the timer to its
//! deadline, so a test says exactly when time runs out, and nothing it
//! checks can be cut short by a slow machine. What a test waits for in real
//! time is still bounded: a wait that never starts fails the test after
//! [`STARTING`], rather than hanging it.
use super::{Moment, Sleep, Timer};
use std::{sync::Mutex, time::Duration};
use tokio::sync::watch;

/// How long, in real time, a test waits for a wait to start.
const STARTING: Duration = Duration::from_secs(5);

/// One wait started on a [`ManualTimer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Wait {
    /// When it started.
    pub(crate) started: Moment,
    /// When it ends.
    pub(crate) deadline: Moment,
}

impl Wait {
    /// How long it was given.
    pub(crate) fn limit(self) -> Duration {
        self.deadline.since(self.started)
    }
}

/// A timer at its origin until the test advances it, recording every wait
/// started on it.
pub(crate) struct ManualTimer {
    now: watch::Sender<Moment>,
    /// Every wait started, in order.
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
    /// Move the timer `by` forward, ending every wait whose deadline that
    /// reaches.
    pub(crate) fn advance(&self, by: Duration) {
        self.now.send_modify(|now| *now = *now + by);
    }
    /// Move the timer forward to `moment`; never back.
    pub(crate) fn advance_to(&self, moment: Moment) {
        self.now.send_modify(|now| *now = (*now).max(moment));
    }
    /// How long after its origin the timer is.
    pub(crate) fn elapsed(&self) -> Duration {
        self.now().since_origin()
    }
    /// Every wait started so far, in order.
    pub(crate) fn waits(&self) -> Vec<Wait> {
        self.waits.lock().unwrap().clone()
    }
    /// The first wait matching `wanted`, by its index from zero, once it
    /// has started.
    ///
    /// # Panics
    /// None has started within [`STARTING`] of real time.
    pub(crate) async fn started(&self, wanted: impl Fn(usize, &Wait) -> bool) -> Wait {
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
                started.changed().await.unwrap();
            }
        };
        match tokio::time::timeout(STARTING, found).await {
            Ok(wait) => wait,
            Err(_) => panic!("no such wait started; waits: {:?}", self.waits()),
        }
    }
    /// The `index`th wait (from zero), once it has started.
    pub(crate) async fn nth(&self, index: usize) -> Wait {
        self.started(|at, _| at == index).await
    }
    /// Once the `index`th wait has started, move the timer to its deadline.
    pub(crate) async fn run_out(&self, index: usize) -> Wait {
        let wait = self.nth(index).await;
        self.advance_to(wait.deadline);
        wait
    }
    /// `operation`, with every wait run out as soon as it starts.
    pub(crate) async fn passing<T>(&self, operation: impl std::future::Future<Output = T>) -> T {
        let mut started = self.started.subscribe();
        let passing = async {
            loop {
                if let Some(latest) = self.waits().iter().map(|wait| wait.deadline).max() {
                    self.advance_to(latest);
                }
                started.changed().await.unwrap();
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
        self.waits.lock().unwrap().push(Wait {
            started: *now.borrow(),
            deadline,
        });
        self.started.send_modify(|started| *started += 1);
        Box::pin(async move {
            // A timer nobody holds any more never moves again.
            if now.wait_for(|now| *now >= deadline).await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }
}
