//! Retain the actual process or pre-start directory until cleanup can be retried.
use super::{AcpConfig, ExecutableUseGuard};
use crate::application::agent_execution::{
    agents::AgentError,
    providers::{CleanupFuture, CleanupReport, CloseOutcome, ProviderCleanup},
};
use crate::infrastructure::process::{ProcessScope, RetainedDirectory};
use std::time::Duration;
use tokio::{runtime::Handle, sync::Mutex, time::sleep};

pub(crate) struct ProcessCleanup {
    state: Mutex<CleanupState>,
    config: AcpConfig,
    runtime: Handle,
}
#[derive(Default)]
struct CleanupState {
    resource: Option<CleanupResource>,
    confirmed: Option<CloseOutcome>,
    executable_use: Option<Box<dyn ExecutableUseGuard>>,
}

enum CleanupResource {
    Process(Box<ProcessScope>),
    Directory(RetainedDirectory),
}
impl ProcessCleanup {
    pub(crate) fn new(config: AcpConfig, executable_use: Box<dyn ExecutableUseGuard>) -> Self {
        Self {
            state: Mutex::new(CleanupState {
                executable_use: Some(executable_use),
                ..CleanupState::default()
            }),
            config,
            runtime: Handle::current(),
        }
    }
    pub(crate) async fn retain(&self, scope: ProcessScope) {
        self.state.lock().await.resource = Some(CleanupResource::Process(Box::new(scope)));
    }
    pub(crate) fn retaining_directory(
        config: AcpConfig,
        directory: RetainedDirectory,
        executable_use: Box<dyn ExecutableUseGuard>,
    ) -> Self {
        Self {
            state: Mutex::new(CleanupState {
                resource: Some(CleanupResource::Directory(directory)),
                confirmed: None,
                executable_use: Some(executable_use),
            }),
            config,
            runtime: Handle::current(),
        }
    }
    pub(crate) fn retaining_use(
        config: AcpConfig,
        executable_use: Box<dyn ExecutableUseGuard>,
    ) -> Self {
        Self {
            state: Mutex::new(CleanupState {
                resource: None,
                confirmed: Some(CloseOutcome { forced: false }),
                executable_use: Some(executable_use),
            }),
            config,
            runtime: Handle::current(),
        }
    }
    pub(crate) async fn confirmed(&self) -> Option<CloseOutcome> {
        let state = self.state.lock().await;
        state
            .executable_use
            .is_none()
            .then_some(state.confirmed)
            .flatten()
    }
    pub(crate) async fn confirm_physical(&self, outcome: CloseOutcome) -> CleanupReport {
        self.state.lock().await.confirmed = Some(outcome);
        self.retry_cleanup().await
    }
    pub(crate) async fn needs_resource(&self) -> bool {
        self.state.lock().await.confirmed.is_none()
    }
}
impl ProviderCleanup for ProcessCleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if state.confirmed.is_none() {
                let Some(resource) = state.resource.as_mut() else {
                    return CleanupReport::unconfirmed(AgentError::CleanupUncertain);
                };
                let outcome = match resource {
                    CleanupResource::Process(scope) => {
                        match scope
                            .cleanup(self.config.shutdown_grace, self.config.kill_timeout)
                            .await
                        {
                            Ok(outcome) => outcome,
                            Err(error) => return CleanupReport::unconfirmed(error),
                        }
                    }
                    CleanupResource::Directory(directory) => {
                        if let Err(error) = directory.release(self.config.kill_timeout).await {
                            return CleanupReport::unconfirmed(error);
                        }
                        CloseOutcome { forced: false }
                    }
                };
                state.confirmed = Some(outcome);
                state.resource.take();
            }
            if let Some(executable_use) = state.executable_use.as_mut() {
                if let Err(error) = executable_use.release() {
                    return CleanupReport::unconfirmed(AgentError::Configuration(format!(
                        "executable use release acknowledgement failed: {error}"
                    )));
                }
                state.executable_use.take();
            }
            if let Some(outcome) = state.confirmed {
                return CleanupReport::confirmed(outcome);
            }
            CleanupReport::unconfirmed(AgentError::CleanupUncertain)
        })
    }
}

impl Drop for ProcessCleanup {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        let mut resource = state.resource.take();
        let mut executable_use = state.executable_use.take();
        if resource.is_none() && executable_use.is_none() {
            return;
        }
        let grace = self.config.shutdown_grace;
        let kill_timeout = self.config.kill_timeout;
        // The worker transfers an uncertain scope before releasing its last Arc.
        // Keep that scope owned after caller loss. This task owns no recovery Arc,
        // so dropping it cannot recursively spawn another cleanup task.
        drop(self.runtime.spawn(async move {
            let mut backoff = Duration::from_millis(100);
            loop {
                if let Some(retained) = resource.as_mut() {
                    let result = match retained {
                        CleanupResource::Process(scope) => scope.cleanup(grace, kill_timeout).await,
                        CleanupResource::Directory(directory) => directory
                            .release(kill_timeout)
                            .await
                            .map(|()| CloseOutcome { forced: false }),
                    };
                    if let Err(error) = result {
                        let retained_directory = match retained {
                            CleanupResource::Process(scope) => scope.retained_directory_path(),
                            CleanupResource::Directory(directory) => Some(directory.path()),
                        };
                        tracing::warn!(
                            %error,
                            ?retained_directory,
                            "retaining abandoned ACP resource until cleanup is confirmed"
                        );
                        sleep(backoff).await;
                        backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
                        continue;
                    }
                    resource.take();
                }
                if let Some(guard) = executable_use.as_mut() {
                    if let Err(error) = guard.release() {
                        tracing::warn!(
                            %error,
                            "retaining abandoned executable use until release is acknowledged"
                        );
                        sleep(backoff).await;
                        backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
                        continue;
                    }
                    executable_use.take();
                }
                break;
            }
        }));
    }
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/sessions/executable_use_cleanup.rs"]
mod executable_use_tests;
