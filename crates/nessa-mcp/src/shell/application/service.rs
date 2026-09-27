use super::ports::{Audit, Runner};
use super::ShellError;
use crate::shell::domain::{RunCause, ShellCommand, StopCause, ToolInvocation};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::watch;

#[derive(Clone)]
pub struct RunRequest {
    pub id: String,
    pub invocation: ToolInvocation,
    pub command: ShellCommand,
    pub cwd: PathBuf,
}
#[derive(Debug, Clone)]
pub struct RunResult {
    pub cause: RunCause,
    pub scope_id: Option<u64>,
    pub process_id: Option<u64>,
    pub os_pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<String>,
    pub forced: Option<bool>,
    pub stdout: String,
    pub stderr: String,
    pub dropped_bytes: u64,
    pub cleanup_verified: bool,
    pub cleanup_error: Option<String>,
    pub output_errors: Vec<String>,
    pub audit_error: Option<String>,
}
#[derive(Clone, Debug)]
pub enum Evidence {
    Admitted,
    Started {
        scope_id: u64,
        process_id: u64,
        os_pid: Option<u32>,
    },
    Finished(RunResult),
}

pub async fn record(audit: &dyn Audit, request: &RunRequest, evidence: Evidence) -> Result<(), ()> {
    let phase = match &evidence {
        Evidence::Admitted => "admitted",
        Evidence::Started { .. } => "started",
        Evidence::Finished(_) => "finished",
    };
    let result = tokio::time::timeout(Duration::from_secs(2), audit.record(request, evidence))
        .await
        .unwrap_or(Err(()));
    if result.is_err() {
        tracing::error!(command_id = %request.id, phase, "shell audit acknowledgement failed");
    }
    result
}
pub struct ShellService {
    runner: Arc<dyn Runner>,
    audit: Arc<dyn Audit>,
}
impl ShellService {
    pub fn new(runner: Arc<dyn Runner>, audit: Arc<dyn Audit>) -> Self {
        Self { runner, audit }
    }
    pub async fn run(
        &self,
        request: &RunRequest,
        stop: watch::Receiver<Option<StopCause>>,
    ) -> Result<RunResult, ShellError> {
        record(self.audit.as_ref(), request, Evidence::Admitted)
            .await
            .map_err(|_| ShellError::AdmissionAuditFailed)?;
        let mut result = self.runner.run(request, stop, self.audit.as_ref()).await;
        if record(
            self.audit.as_ref(),
            request,
            Evidence::Finished(result.clone()),
        )
        .await
        .is_err()
        {
            result.audit_error = Some("completion audit failed".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "../../../tests/shell/application/service.rs"]
mod tests;
