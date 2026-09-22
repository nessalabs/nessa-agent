//! Retain the actual process scope until a failed teardown can be retried.
use super::AcpConfig;
use crate::application::agent_execution::{
    agents::AgentError,
    providers::{CleanupFuture, CleanupReport, CloseOutcome, ProviderCleanup},
};
use crate::infrastructure::process::ProcessScope;
use std::time::Duration;
use tokio::{runtime::Handle, sync::Mutex, time::sleep};

pub(crate) struct ProcessCleanup {
    state: Mutex<CleanupState>,
    config: AcpConfig,
    runtime: Handle,
}
#[derive(Default)]
struct CleanupState {
    scope: Option<ProcessScope>,
    confirmed: Option<CloseOutcome>,
}
impl ProcessCleanup {
    pub(crate) fn new(config: AcpConfig) -> Self {
        Self {
            state: Mutex::new(CleanupState::default()),
            config,
            runtime: Handle::current(),
        }
    }
    pub(crate) async fn retain(&self, scope: ProcessScope) {
        self.state.lock().await.scope = Some(scope);
    }
    pub(crate) async fn confirmed(&self) -> Option<CloseOutcome> {
        self.state.lock().await.confirmed
    }
}
impl ProviderCleanup for ProcessCleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if let Some(outcome) = state.confirmed {
                return CleanupReport::confirmed(outcome);
            }
            let Some(scope) = state.scope.as_mut() else {
                return CleanupReport::unconfirmed(AgentError::CleanupUncertain);
            };
            let outcome = match scope
                .cleanup(self.config.shutdown_grace, self.config.kill_timeout)
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => return CleanupReport::unconfirmed(error),
            };
            state.confirmed = Some(outcome);
            state.scope.take();
            CleanupReport::confirmed(outcome)
        })
    }
}

impl Drop for ProcessCleanup {
    fn drop(&mut self) {
        let Some(mut scope) = self.state.get_mut().scope.take() else {
            return;
        };
        let grace = self.config.shutdown_grace;
        let kill_timeout = self.config.kill_timeout;
        // The worker transfers an uncertain scope before releasing its last Arc.
        // Keep that scope owned after caller loss. This task owns no recovery Arc,
        // so dropping it cannot recursively spawn another cleanup task.
        drop(self.runtime.spawn(async move {
            let mut backoff = Duration::from_millis(100);
            loop {
                match scope.cleanup(grace, kill_timeout).await {
                    Ok(_) => break,
                    Err(error) => tracing::warn!(
                        %error,
                        retained_directory = ?scope.retained_directory_path(),
                        "retaining abandoned ACP process until cleanup is confirmed"
                    ),
                }
                sleep(backoff).await;
                backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
            }
        }));
    }
}
