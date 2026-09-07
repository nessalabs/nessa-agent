use crate::app::dependencies::RuntimeDependencies;
use crate::app::ports::Clock;
use crate::env::{Environment, Stage};
use std::sync::Arc;

/// Shared runtime state wired once at composition root and passed to entrypoints.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    version: &'static str,
    stage: Stage,
    clock: Arc<dyn Clock>,
}

impl AppState {
    pub fn from_environment(config: &Environment) -> Self {
        Self::with_dependencies(config, RuntimeDependencies::default())
    }

    pub fn with_dependencies(config: &Environment, dependencies: RuntimeDependencies) -> Self {
        Self {
            inner: Arc::new(Inner {
                version: config.version,
                stage: config.stage,
                clock: dependencies.clock,
            }),
        }
    }

    pub fn version(&self) -> &str {
        self.inner.version
    }

    pub fn stage(&self) -> &str {
        self.inner.stage.as_str()
    }

    pub fn uptime_ms(&self) -> u64 {
        self.inner.clock.elapsed_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{MockEnv, STAGE};

    #[test]
    fn builds_from_environment() {
        let config = Environment::load(&MockEnv::new().set(STAGE, "ci")).expect("config");
        let state = AppState::from_environment(&config);
        assert_eq!(state.stage(), "ci");
    }
}
