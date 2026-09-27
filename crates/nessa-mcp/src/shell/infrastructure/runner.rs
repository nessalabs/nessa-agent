use crate::shell::{
    application::{record, Audit, Evidence, RunRequest, RunResult, Runner, Task},
    domain::{RunCause, StopCause},
};
use shepherd::{
    EnvPolicy, GracePeriod, OutputMode, OutputStream, ProcessId, ProcessSpec, ProcessSupervisor,
    TerminateOptions,
};
use std::{ffi::OsString, time::Duration};
use tokio::sync::watch;

pub struct ShepherdRunner {
    supervisor: ProcessSupervisor,
    environment: Vec<(OsString, OsString)>,
}
impl ShepherdRunner {
    pub fn new(supervisor: ProcessSupervisor, environment: Vec<(OsString, OsString)>) -> Self {
        Self {
            supervisor,
            environment,
        }
    }
}
impl Runner for ShepherdRunner {
    fn run<'a>(
        &'a self,
        request: &'a RunRequest,
        mut stop: watch::Receiver<Option<StopCause>>,
        audit: &'a dyn Audit,
    ) -> Task<'a, RunResult> {
        Box::pin(async move {
            let mut result = RunResult {
                cause: RunCause::SpawnFailed,
                scope_id: None,
                process_id: None,
                os_pid: None,
                exit_code: None,
                exit_signal: None,
                forced: None,
                stdout: String::new(),
                stderr: String::new(),
                dropped_bytes: 0,
                cleanup_verified: false,
                cleanup_error: None,
                output_errors: Vec::new(),
                audit_error: None,
            };
            // A preexisting stop is authoritative before any process is admitted.
            if let Some(cause) = *stop.borrow() {
                result.cause = RunCause::Stopped(cause);
                result.cleanup_verified = true;
                return result;
            }
            let supervisor = &self.supervisor;
            let spec = ProcessSpec::new("/bin/bash")
                .arg("--noprofile")
                .arg("--norc")
                .arg("-c")
                .arg(request.command.text())
                .cwd(&request.cwd)
                .env(EnvPolicy::Clear(self.environment.clone()))
                .output(OutputMode::Capture {
                    buffer_bytes: 65536,
                    tail_bytes: 4096,
                });
            let opts = TerminateOptions {
                grace: GracePeriod::new(Duration::from_millis(200)),
                force_timeout: Some(Duration::from_secs(1)),
            };
            let scope = supervisor.with_scope_options(vec![], opts, |scope| async move {
                result.scope_id = Some(scope.id().get());
                if let Some(cause) = *stop.borrow() { result.cause = RunCause::Stopped(cause); return (result, None); }
                let deadline = tokio::time::Instant::now() + request.command.timeout();
                let pid = match scope.spawn(spec).await { Ok(pid) => pid, Err(_) => return (result, None) };
                result.process_id = Some(pid.get()); result.os_pid = supervisor.os_pid(pid);
                let output = scope.take_output(pid);
                if record(audit, request, Evidence::Started { scope_id: scope.id().get(), process_id: pid.get(), os_pid: result.os_pid }).await.is_err() {
                    result.cause = RunCause::AuditFailed;
                    result.audit_error = Some("started audit failed".into());
                } else {
                    result.cause = tokio::select! {
                        biased;
                        _ = async { if stop.borrow().is_none() { let _ = stop.changed().await; } } => RunCause::Stopped(stop.borrow().unwrap_or(StopCause::ClientClosed)),
                        exit = scope.wait(pid) => match exit { Ok(exit) => { result.exit_code = exit.code; RunCause::Exited }, Err(_) => RunCause::WaitFailed },
                        _ = tokio::time::sleep_until(deadline) => RunCause::Timeout,
                    };
                }
                (result, output)
            }).await;
            let (mut result, output) = scope
                .result
                .expect("empty initial process list cannot fail spawn");
            result.cleanup_verified = scope
                .termination
                .as_ref()
                .is_ok_and(|report| report.all_verified());
            result.cleanup_error = match scope.termination {
                Ok(_) => None,
                Err(error) => Some(error.to_string()),
            };
            // Preserve the physical exit independently of the local stop/timeout cause.
            if let Some(pid) = result.process_id.filter(|_| result.cleanup_verified) {
                if let Ok(exit) = supervisor.wait(ProcessId::new(pid)).await {
                    result.exit_code = exit.code;
                    result.exit_signal = exit.signal.map(|signal| format!("{signal:?}"));
                    result.forced = Some(exit.forced);
                }
            }
            if let Some(output) = output {
                let snapshot = output.read();
                result.dropped_bytes = snapshot.dropped_bytes;
                result.output_errors = snapshot.errors;
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                for chunk in snapshot.chunks {
                    match chunk.stream {
                        OutputStream::Stdout => stdout.extend(chunk.bytes),
                        OutputStream::Stderr => stderr.extend(chunk.bytes),
                    }
                }
                result.stdout = String::from_utf8_lossy(&stdout).into_owned();
                result.stderr = String::from_utf8_lossy(&stderr).into_owned();
            }
            result
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/shell/infrastructure/runner.rs"]
mod tests;
