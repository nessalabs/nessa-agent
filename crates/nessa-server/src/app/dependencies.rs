use crate::app::ports::Clock;
use std::{sync::Arc, time::Instant};

/// Typed constructor inputs. Clone shares one application's dependencies.
#[derive(Clone)]
pub struct RuntimeDependencies {
    pub clock: Arc<dyn Clock>,
}

impl Default for RuntimeDependencies {
    fn default() -> Self {
        Self {
            clock: Arc::new(MonotonicClock(Instant::now())),
        }
    }
}

struct MonotonicClock(Instant);
impl Clock for MonotonicClock {
    fn elapsed_ms(&self) -> u64 {
        self.0.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::state::AppState,
        env::{Environment, MockEnv},
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FakeClock(AtomicU64);
    impl Clock for FakeClock {
        fn elapsed_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn injected_clock_is_shared_by_clones_but_not_other_applications() {
        let config = Environment::load(&MockEnv::new()).unwrap();
        let clock = Arc::new(FakeClock(AtomicU64::new(42)));
        let state = AppState::with_dependencies(
            &config,
            RuntimeDependencies {
                clock: clock.clone(),
            },
        );
        let other = AppState::with_dependencies(
            &config,
            RuntimeDependencies {
                clock: Arc::new(FakeClock(AtomicU64::new(7))),
            },
        );
        let cloned = state.clone();
        clock.0.store(99, Ordering::SeqCst);
        assert_eq!(cloned.uptime_ms(), 99);
        assert_eq!(other.uptime_ms(), 7);
    }
}
