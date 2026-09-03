use crate::env::Environment;
use std::sync::Arc;
use std::time::Instant;

/// Shared runtime state wired once at composition root and passed to entrypoints.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    auth_token: String,
    version: &'static str,
    stage: String,
    started_at: Instant,
}

impl AppState {
    pub fn from_environment(config: &Environment) -> Self {
        Self {
            inner: Arc::new(Inner {
                auth_token: config.auth_token.clone(),
                version: config.version,
                stage: config.stage.as_str().to_string(),
                started_at: Instant::now(),
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
        &self.inner.stage
    }

    pub fn uptime_ms(&self) -> u64 {
        self.inner.started_at.elapsed().as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{MockEnv, STAGE, TOKEN};

    #[test]
    fn builds_from_environment() {
        let config = Environment::load(
            &MockEnv::new()
                .set(STAGE, "ci")
                .set(TOKEN, "secret"),
        )
        .expect("config");
        let state = AppState::from_environment(&config);
        assert_eq!(state.auth_token(), "secret");
        assert_eq!(state.stage(), "ci");
    }
}
