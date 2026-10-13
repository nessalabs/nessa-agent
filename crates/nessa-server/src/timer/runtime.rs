//! The runtime's own clock as a [`Timer`].
use super::{Moment, Sleep, Timer};

/// Tokio's clock, measured from when this value was made.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TokioTimer {
    origin: tokio::time::Instant,
}

impl TokioTimer {
    /// A timer whose origin is now.
    pub(crate) fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl Default for TokioTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer for TokioTimer {
    fn now(&self) -> Moment {
        Moment::from_origin(self.origin.elapsed())
    }
    fn sleep_until(&self, deadline: Moment) -> Sleep {
        match self.origin.checked_add(deadline.since_origin()) {
            Some(deadline) => Box::pin(tokio::time::sleep_until(deadline)),
            // Past what the runtime can represent: never reached.
            None => Box::pin(std::future::pending()),
        }
    }
}
