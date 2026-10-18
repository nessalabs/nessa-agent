//! Test-owned logical time for lease fetch ordering, not a production clock.
//! A parked blocking task inhibits Tokio auto-advance during real admission I/O
//! (Tokio's `time::pause` contract). No CPU spin or filesystem timing promise.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use tokio::{sync::oneshot, task::JoinHandle};

/// The fetch fixture's existing outer watchdog, measured in real time while
/// logical time is paused; expiration is a failure, not permission to pass.
pub(super) const FETCH_TEST_WATCHDOG: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FetchClockFailure {
    WatchdogExpired,
    ClockAdvanced {
        expected: Duration,
        actual: Duration,
    },
}

pub(super) struct FrozenFetchTime {
    release: Option<mpsc::Sender<()>>,
    keeper: Option<JoinHandle<()>>,
    expired: Arc<AtomicBool>,
    wall_start: Instant,
    wall_wait: Duration,
    logical_start: tokio::time::Instant,
}

impl FrozenFetchTime {
    /// Freeze only after real harness setup. The keeper has started before
    /// pause, and has a finite real-time wait even if a test fails to finish.
    pub(super) async fn freeze(wall_wait: Duration) -> Self {
        let (release, held) = mpsc::channel();
        let (started, ready) = oneshot::channel();
        let expired = Arc::new(AtomicBool::new(false));
        let timed_out = expired.clone();
        let wall_start = Instant::now();
        let keeper = tokio::task::spawn_blocking(move || {
            let _ = started.send(());
            if matches!(
                held.recv_timeout(wall_wait),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                timed_out.store(true, Ordering::SeqCst);
            }
        });
        ready.await.expect("the fetch clock keeper started");
        tokio::time::pause();
        Self {
            release: Some(release),
            keeper: Some(keeper),
            expired,
            wall_start,
            wall_wait,
            logical_start: tokio::time::Instant::now(),
        }
    }

    /// Resume before awaiting the keeper or any physical final save. The
    /// result must be checked even when the production fetch result is expected.
    /// Return the remaining wall watchdog, so awaiting a final save cannot
    /// restart the original per-call budget.
    pub(super) async fn finish(
        mut self,
        expected: Duration,
    ) -> Result<Duration, FetchClockFailure> {
        let actual = self.logical_start.elapsed();
        self.resume();
        self.keeper
            .take()
            .unwrap()
            .await
            .expect("the fetch clock keeper ended");
        let remaining = self.wall_wait.saturating_sub(self.wall_start.elapsed());
        if self.expired.load(Ordering::SeqCst) || remaining.is_zero() {
            Err(FetchClockFailure::WatchdogExpired)
        } else if actual != expected {
            Err(FetchClockFailure::ClockAdvanced { expected, actual })
        } else {
            Ok(remaining)
        }
    }

    fn resume(&mut self) {
        if let Some(release) = self.release.take() {
            tokio::time::resume();
            drop(release);
        }
    }
}

impl Drop for FrozenFetchTime {
    fn drop(&mut self) {
        // Cleanup only: never assert during another assertion's unwind. Dropping
        // the sender releases the parked task; recv_timeout also bounds it.
        self.resume();
    }
}

#[tokio::test]
async fn an_expired_wall_watchdog_cannot_turn_auto_advance_into_a_passing_timeout() {
    let clock = FrozenFetchTime::freeze(Duration::from_millis(20)).await;
    // No external work releases this keeper. Its real watchdog expires, then
    // Tokio can advance; even the expected logical duration is not a pass.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        clock.finish(Duration::from_secs(1)).await,
        Err(FetchClockFailure::WatchdogExpired)
    );
    tokio::time::pause(); // finish resumed and released its sole clock ownership.
    tokio::time::resume();
}

#[tokio::test]
async fn dropping_a_fetch_clock_during_unwind_resumes_and_releases_it() {
    let (ending, ended) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
        let keeper = clock.keeper.take().unwrap();
        let _ = ending.send((keeper, clock.expired.clone()));
        panic!("synthetic assertion while a fetch clock is held");
    });
    let (keeper, expired) = ended.await.unwrap();
    assert!(task.await.unwrap_err().is_panic());
    keeper.await.unwrap();
    assert!(
        !expired.load(Ordering::SeqCst),
        "unwind released the keeper before its watchdog"
    );
    tokio::time::pause(); // Drop already resumed; a leaked pause would panic here.
    tokio::time::resume();
}

#[tokio::test]
async fn an_explicit_fetch_wait_expires_the_actual_timer_with_auto_advance_inhibited() {
    let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
    let (started, ready) = oneshot::channel();
    let timer = tokio::spawn(async move {
        let sleep = tokio::time::sleep(Duration::from_secs(5));
        started.send(()).unwrap();
        sleep.await;
    });
    ready.await.unwrap();
    assert!(!timer.is_finished());
    tokio::time::advance(Duration::from_secs(5)).await;
    clock.finish(Duration::from_secs(5)).await.unwrap();
    timer.await.unwrap();
}

#[tokio::test]
async fn a_fetch_clock_rejects_a_different_logical_duration_and_releases_ownership() {
    let clock = FrozenFetchTime::freeze(FETCH_TEST_WATCHDOG).await;
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        clock.finish(Duration::ZERO).await,
        Err(FetchClockFailure::ClockAdvanced {
            expected: Duration::ZERO,
            actual: Duration::from_secs(1),
        })
    );
    tokio::time::pause(); // Even a rejected duration releases clock ownership.
    tokio::time::resume();
}
