//! Retain the actual process or pre-start directory until cleanup can be retried.
use super::AcpConfig;
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
}

enum CleanupResource {
    Process(ProcessScope),
    Directory(RetainedDirectory),
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
        self.state.lock().await.resource = Some(CleanupResource::Process(scope));
    }
    pub(crate) fn retaining_directory(config: AcpConfig, directory: RetainedDirectory) -> Self {
        Self {
            state: Mutex::new(CleanupState {
                resource: Some(CleanupResource::Directory(directory)),
                confirmed: None,
            }),
            config,
            runtime: Handle::current(),
        }
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
            CleanupReport::confirmed(outcome)
        })
    }
}

impl Drop for ProcessCleanup {
    fn drop(&mut self) {
        let Some(mut resource) = self.state.get_mut().resource.take() else {
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
                let result = match &mut resource {
                    CleanupResource::Process(scope) => scope.cleanup(grace, kill_timeout).await,
                    CleanupResource::Directory(directory) => directory
                        .release(kill_timeout)
                        .await
                        .map(|()| CloseOutcome { forced: false }),
                };
                match result {
                    Ok(_) => break,
                    Err(error) => {
                        let retained_directory = match &resource {
                            CleanupResource::Process(scope) => scope.retained_directory_path(),
                            CleanupResource::Directory(directory) => Some(directory.path()),
                        };
                        tracing::warn!(
                            %error,
                            ?retained_directory,
                            "retaining abandoned ACP resource until cleanup is confirmed"
                        );
                    }
                }
                sleep(backoff).await;
                backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
            }
        }));
    }
}
