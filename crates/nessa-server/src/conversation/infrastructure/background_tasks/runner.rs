//! Shepherd owns each command scope, independent of the provider's lease.
use crate::conversation::{
    application::background_tasks::{BackgroundRun, BackgroundRunner, BackgroundTaskError},
    domain::{BackgroundCommand, TaskCause, TaskOutcome, TaskStop},
};
use shepherd::{
    BackgroundTask, BackgroundTaskCause, BackgroundTaskOptions, Clock, EnvPolicy, GracePeriod,
    OutputStream, ProcessSpec, ProcessSupervisor, TerminateOptions,
};
use std::{ffi::OsString, num::NonZeroUsize, sync::Arc, time::Duration};
use tokio::sync::watch;

pub struct ShepherdBackgroundTasks {
    supervisor: ProcessSupervisor,
    environment: Vec<(OsString, OsString)>,
    clock: Arc<dyn Clock>,
}
impl ShepherdBackgroundTasks {
    pub fn new(
        supervisor: ProcessSupervisor,
        environment: Vec<(OsString, OsString)>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            supervisor,
            environment,
            clock,
        }
    }
}
impl BackgroundRunner for ShepherdBackgroundTasks {
    fn run(
        &self,
        command: BackgroundCommand,
        workspace: String,
        mut stop: watch::Receiver<Option<TaskStop>>,
    ) -> BackgroundRun<'_, TaskOutcome> {
        Box::pin(async move {
            let mut result = TaskOutcome {
                cause: TaskCause::SpawnFailed,
                scope: None,
                process: None,
                os_pid: None,
                exit_code: None,
                signal: None,
                cleanup_verified: true,
                stdout: String::new(),
                stderr: String::new(),
                omitted_bytes: 0,
                output_complete: true,
                error: None,
            };
            if stop.borrow().is_some() {
                result.cause = TaskCause::Cancelled;
                return result;
            }
            let spec = ProcessSpec::new("/bin/bash")
                .arg("--noprofile")
                .arg("--norc")
                .arg("-c")
                .arg(command.text())
                .cwd(workspace)
                .env(EnvPolicy::Clear(self.environment.clone()));
            let options = BackgroundTaskOptions {
                timeout: Duration::from_secs(command.timeout_seconds()),
                output_bytes: NonZeroUsize::new(TaskOutcome::MAX_OUTPUT_BYTES)
                    .expect("nonzero background output bound"),
                termination: TerminateOptions {
                    grace: GracePeriod::new(Duration::from_millis(200)),
                    force_timeout: Some(Duration::from_secs(2)),
                },
            };
            let task = match BackgroundTask::start(
                self.supervisor.clone(),
                spec,
                options,
                self.clock.clone(),
            ) {
                Ok(task) => task,
                Err(error) => {
                    result.error = Some(error.to_string());
                    return result;
                }
            };
            let settled = tokio::select! {
                outcome = task.wait() => outcome,
                _ = async { if stop.borrow().is_none() { let _ = stop.changed().await; } } => { task.cancel(); task.wait().await },
            };
            result.scope = Some(settled.scope.get());
            result.process = settled.process.map(|process| process.get());
            result.os_pid = settled.os_pid;
            result.cause = match &settled.cause {
                BackgroundTaskCause::Exited => TaskCause::Exited,
                BackgroundTaskCause::Cancelled => TaskCause::Cancelled,
                BackgroundTaskCause::TimedOut => TaskCause::TimedOut,
                BackgroundTaskCause::SpawnFailed(error) => {
                    result.error = Some(error.to_string());
                    TaskCause::SpawnFailed
                }
                BackgroundTaskCause::WorkerFailed => TaskCause::WorkerFailed,
            };
            result.cleanup_verified = settled.cleanup_verified();
            if let Some(exit) = &settled.exit {
                result.exit_code = exit.code;
                result.signal = exit.signal.map(|signal| format!("{signal:?}"));
            }
            if let Err(error) = &settled.cleanup {
                result.error = Some(format!(
                    "{}Cleanup: {error}",
                    result.error.map(|s| format!("{s}; ")).unwrap_or_default()
                ));
            }
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            for chunk in &settled.output.tail {
                match chunk.stream {
                    OutputStream::Stdout => stdout.extend_from_slice(&chunk.bytes),
                    OutputStream::Stderr => stderr.extend_from_slice(&chunk.bytes),
                }
            }
            result.stdout = String::from_utf8_lossy(&stdout).into_owned();
            result.stderr = String::from_utf8_lossy(&stderr).into_owned();
            result.omitted_bytes = settled.output.omitted_bytes;
            result.output_complete = settled.output.stdout_closed
                && settled.output.stderr_closed
                && settled.output.errors.is_empty();
            if !settled.output.errors.is_empty() {
                result.error = Some(format!(
                    "{}Output: {}",
                    result.error.map(|s| format!("{s}; ")).unwrap_or_default(),
                    settled.output.errors.join("; ")
                ));
            }
            result
        })
    }
    fn shutdown(&self) -> BackgroundRun<'_, Result<(), BackgroundTaskError>> {
        Box::pin(async move {
            let report = self
                .supervisor
                .shutdown()
                .await
                .map_err(|_| BackgroundTaskError::CleanupUnverified)?;
            if report.scopes.iter().all(|scope| scope.all_verified()) {
                Ok(())
            } else {
                Err(BackgroundTaskError::CleanupUnverified)
            }
        })
    }
}
