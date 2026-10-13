//! Platform clock adapters supplied at the foreign composition entry point.
use nessa_auth::application::ports::Clock as WallClock;
use nessa_protocol::clock::Clock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
pub(crate) struct Wall;
impl WallClock for Wall {
    fn unix_milliseconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |value| {
                u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
            })
    }
}
pub(crate) struct Monotonic(Instant);
impl Monotonic {
    pub(crate) fn new() -> Self {
        Self(Instant::now())
    }
}
impl Clock for Monotonic {
    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}
