//! The one place a process reads the time or waits for it, for code that is
//! not handed a [`Timer`](crate::Timer).
//!
//! Async code reads and waits on tokio's clock, so a test that pauses tokio
//! time (`#[tokio::test(start_paused = true)]`) moves every deadline the code
//! under test measures, instead of waiting for them on the wall. Outside a
//! paused test it is the same monotonic clock `std` reads.
//!
//! Code on a thread of its own, which no runtime drives, reads and waits on
//! [`os`]: a paused runtime clock would never move for it. So does code that
//! keeps a `std` instant to compare with one a library hands back.
//!
//! The wall clock ([`wall_now`]) is for timestamps people read and records
//! keep, never for measuring a wait.
use std::time::{Duration, SystemTime};
use tokio::time::{Instant, Sleep};

/// The current moment on the runtime's clock.
pub fn now() -> Instant {
    Instant::now()
}

/// Resolves once `period` has passed on the runtime's clock.
pub fn sleep(period: Duration) -> Sleep {
    tokio::time::sleep(period)
}

/// Resolves once the runtime's clock reaches `deadline`.
pub fn sleep_until(deadline: Instant) -> Sleep {
    tokio::time::sleep_until(deadline)
}

/// The wall-clock time now.
pub fn wall_now() -> SystemTime {
    SystemTime::now()
}

/// The operating system's clock, for code on a thread no runtime drives (a
/// blocking poll loop, a process wait) and for `std` instants. It never
/// pauses, so a paused test cannot strand a thread waiting on it.
pub mod os {
    use std::time::{Duration, Instant};

    /// The current moment on the operating system's monotonic clock.
    pub fn now() -> Instant {
        Instant::now()
    }

    /// Blocks this thread for `period`.
    pub fn sleep(period: Duration) {
        std::thread::sleep(period)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of reading through here: a paused test moves the clock
    /// the code reads, and a wait on it ends without real time passing.
    #[tokio::test(start_paused = true)]
    async fn a_paused_test_moves_the_clock_code_reads() {
        let started = now();
        let real = os::now();
        sleep(Duration::from_secs(3600)).await;
        assert_eq!(now() - started, Duration::from_secs(3600));
        assert!(real.elapsed() < Duration::from_secs(60));
        sleep_until(now() + Duration::from_secs(1)).await;
        assert_eq!(now() - started, Duration::from_secs(3601));
    }
}
