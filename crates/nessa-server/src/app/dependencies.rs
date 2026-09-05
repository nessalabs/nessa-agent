use crate::app::ports::UptimeClock;
use std::{sync::Arc, time::Instant};

/// Typed constructor inputs. Clone shares one application's dependencies.
#[derive(Clone)]
pub struct RuntimeDependencies {
    pub uptime_clock: Arc<dyn UptimeClock>,
}

impl Default for RuntimeDependencies {
    fn default() -> Self {
        Self {
            uptime_clock: Arc::new(MonotonicUptimeClock(Instant::now())),
        }
    }
}

struct MonotonicUptimeClock(Instant);
impl UptimeClock for MonotonicUptimeClock {
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

    struct FakeUptimeClock(AtomicU64);
    impl UptimeClock for FakeUptimeClock {
        fn elapsed_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn injected_clock_is_shared_by_clones_but_not_other_applications() {
        let config = Environment::load(&MockEnv::new()).unwrap();
        let clock = Arc::new(FakeUptimeClock(AtomicU64::new(42)));
        let state = AppState::with_dependencies(
            &config,
            RuntimeDependencies {
                uptime_clock: clock.clone(),
            },
        );
        let other = AppState::with_dependencies(
            &config,
            RuntimeDependencies {
                uptime_clock: Arc::new(FakeUptimeClock(AtomicU64::new(7))),
            },
        );
        let cloned = state.clone();
        clock.0.store(99, Ordering::SeqCst);
        assert_eq!(cloned.uptime_ms(), 99);
        assert_eq!(other.uptime_ms(), 7);
    }
}
