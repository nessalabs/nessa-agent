//! Executable clock adapters. Authentication/cache wall time and socket elapsed time
//! have separate ports and separate meanings.
use nessa_auth::application::ports::Clock as WallClock;
use nessa_protocol::clock::Clock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
pub(super) struct SystemClock;
impl WallClock for SystemClock {
    fn unix_milliseconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }
}
pub(super) struct MonotonicClock(Instant);
impl MonotonicClock {
    pub(super) fn new() -> Self {
        Self(Instant::now())
    }
}
impl Clock for MonotonicClock {
    fn elapsed_ms(&self) -> u64 {
        self.0.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
}
