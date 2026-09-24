//! Retain the actual process or pre-start directory until cleanup can be retried.
use super::AcpConfig;
use crate::application::agent_execution::{
    agents::AgentError,
    providers::{
        CleanupFuture, CleanupReport, CloseOutcome, ExecutableUseGuard, ProviderCleanup,
        ResourceCleanup,
    },
};
use crate::infrastructure::process::{ProcessScope, RetainedDirectory};
use std::time::Duration;
use tokio::{runtime::Handle, sync::Mutex, time::sleep};

pub(crate) struct ProcessCleanup {
    state: Mutex<CleanupState>,
    config: AcpConfig,
    runtime: Handle,
}

struct CleanupState {
    physical: PhysicalCleanup,
    executable_use: Option<Box<dyn ExecutableUseGuard>>,
}

enum PhysicalCleanup {
    Unknown,
    Retained(CleanupResource),
    Confirmed(CloseOutcome),
}

enum CleanupResource {
    Process(Box<ProcessScope>),
    Directory(RetainedDirectory),
}

impl ProcessCleanup {
    pub(crate) fn new(config: AcpConfig, executable_use: Box<dyn ExecutableUseGuard>) -> Self {
        Self {
            state: Mutex::new(CleanupState {
                physical: PhysicalCleanup::Unknown,
                executable_use: Some(executable_use),
            }),
            config,
            runtime: Handle::current(),
        }
    }

    pub(crate) async fn retain(&self, scope: ProcessScope) {
        let mut state = self.state.lock().await;
        assert!(
            matches!(&state.physical, PhysicalCleanup::Unknown),
            "a process cleanup owner can retain only one spawned process"
        );
        state.physical = PhysicalCleanup::Retained(CleanupResource::Process(Box::new(scope)));
    }

    pub(crate) fn retaining_directory(
        config: AcpConfig,
        directory: RetainedDirectory,
        executable_use: Box<dyn ExecutableUseGuard>,
    ) -> Self {
        Self {
            state: Mutex::new(CleanupState {
                physical: PhysicalCleanup::Retained(CleanupResource::Directory(directory)),
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
                physical: PhysicalCleanup::Confirmed(CloseOutcome { forced: false }),
                executable_use: Some(executable_use),
            }),
            config,
            runtime: Handle::current(),
        }
    }

    pub(crate) async fn confirmed(&self) -> Option<CloseOutcome> {
        let state = self.state.lock().await;
        match (&state.physical, state.executable_use.is_none()) {
            (PhysicalCleanup::Confirmed(outcome), true) => Some(*outcome),
            _ => None,
        }
    }

    pub(crate) async fn confirm_physical(&self, outcome: CloseOutcome) -> CleanupReport {
        self.state.lock().await.physical = PhysicalCleanup::Confirmed(outcome);
        self.retry_cleanup().await
    }

    pub(crate) async fn needs_resource(&self) -> bool {
        !matches!(
            &self.state.lock().await.physical,
            PhysicalCleanup::Confirmed(_)
        )
    }
}

impl ProviderCleanup for ProcessCleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            advance_cleanup(
                &mut state,
                self.config.shutdown_grace,
                self.config.kill_timeout,
            )
            .await
        })
    }
}

async fn advance_cleanup(
    state: &mut CleanupState,
    grace: Duration,
    kill_timeout: Duration,
) -> CleanupReport {
    let outcome = match &mut state.physical {
        PhysicalCleanup::Unknown => {
            return CleanupReport::unconfirmed(AgentError::CleanupUncertain);
        }
        PhysicalCleanup::Retained(resource) => {
            let result = match resource {
                CleanupResource::Process(scope) => scope.cleanup(grace, kill_timeout).await,
                CleanupResource::Directory(directory) => directory
                    .release(kill_timeout)
                    .await
                    .map(|()| CloseOutcome { forced: false }),
            };
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(error) => return CleanupReport::unconfirmed(error),
            };
            state.physical = PhysicalCleanup::Confirmed(outcome);
            outcome
        }
        PhysicalCleanup::Confirmed(outcome) => *outcome,
    };

    if let Some(executable_use) = state.executable_use.as_mut() {
        if let Err(error) = executable_use.release() {
            return CleanupReport::new(
                ResourceCleanup::ReleasePending {
                    physical: outcome,
                    failure: AgentError::Configuration(format!(
                        "executable use release acknowledgement failed: {error}"
                    )),
                },
                Ok(()),
            );
        }
        state.executable_use.take();
    }
    CleanupReport::confirmed(outcome)
}

impl Drop for ProcessCleanup {
    fn drop(&mut self) {
        let state = std::mem::replace(
            self.state.get_mut(),
            CleanupState {
                physical: PhysicalCleanup::Unknown,
                executable_use: None,
            },
        );
        if matches!(&state.physical, PhysicalCleanup::Confirmed(_))
            && state.executable_use.is_none()
        {
            return;
        }
        let grace = self.config.shutdown_grace;
        let kill_timeout = self.config.kill_timeout;
        // This task is the sole owner after caller loss. Cancellation drops its
        // retained resource and durable use guard without acknowledging release.
        drop(self.runtime.spawn(async move {
            let mut state = state;
            let mut backoff = Duration::from_millis(100);
            loop {
                let report = advance_cleanup(&mut state, grace, kill_timeout).await;
                if report.is_confirmed() {
                    break;
                }
                tracing::warn!(
                    resources = ?report.resources(),
                    "retaining abandoned ACP cleanup ownership until release is confirmed"
                );
                sleep(backoff).await;
                backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
            }
        }));
    }
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/sessions/executable_use_cleanup.rs"]
mod executable_use_tests;
