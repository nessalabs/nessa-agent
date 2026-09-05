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
    auth_token: String,
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
                auth_token: config.auth_token.clone(),
                version: config.version,
                stage: config.stage,
                clock: dependencies.clock,
            }),
        }
    }

    pub fn auth_token(&self) -> &str {
        &self.inner.auth_token
    }

    pub fn version(&self) -> &str {
        self.inner.version
    }

    pub fn stage(&self) -> &str {
        self.inner.stage.as_str()
    }

    /// Whether `server.ping` was composed for this process (ADR 0006: `dev` only).
    pub fn offers_server_ping(&self) -> bool {
        self.inner.stage == Stage::Dev
    }

    pub fn uptime_ms(&self) -> u64 {
        self.inner.clock.elapsed_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{MockEnv, STAGE, TOKEN};

    #[test]
    fn builds_from_environment() {
        let config = Environment::load(&MockEnv::new().set(STAGE, "ci").set(TOKEN, "secret"))
            .expect("config");
        let state = AppState::from_environment(&config);
        assert_eq!(state.auth_token(), "secret");
        assert_eq!(state.stage(), "ci");
        assert!(!state.offers_server_ping());
    }

    #[test]
    fn offers_ping_only_on_dev() {
        let dev =
            AppState::from_environment(&Environment::load(&MockEnv::new()).expect("defaults"));
        assert!(dev.offers_server_ping());
    }
}
