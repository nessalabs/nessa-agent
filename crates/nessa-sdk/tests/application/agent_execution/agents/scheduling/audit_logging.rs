//! Adapter diagnostics remain caller-owned at the shared audit logging boundary.
use super::{prepared, test_admission};
use crate::application::agent_execution::{
    agents::{
        audit_observation::AuditObservationFailure, AgentError, AgentFuture, ProviderDiagnostic,
    },
    executions::{ExecutionAudit, ExecutionAuditRecord},
};
use crate::infrastructure::session_storage::InMemoryStorage;
use std::{
    env,
    io::{self, Write},
    process::Command,
    sync::{Arc, Mutex},
};
use tokio::runtime::Builder;

const SENTINEL: &str = "private-audit-diagnostic-sentinel";
const CHILD: &str = "NESSA_AUDIT_LOGGING_CHILD";

#[derive(Clone, Default)]
struct Warnings(Arc<Mutex<Vec<u8>>>);
impl Write for Warnings {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct RefusingAudit(AgentError);
impl ExecutionAudit for RefusingAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let error = self.0.clone();
        Box::pin(async move { Err(error) })
    }
}
struct PanickingAudit;
impl ExecutionAudit for PanickingAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { panic!("audit poll fault") })
    }
}

#[test]
fn audit_error_diagnostics_stay_with_the_caller_and_out_of_tracing() {
    // As in caller_wakers' tracing capture: isolate process-wide callsite
    // interest from parallel tests, with a current-thread subscriber/runtime.
    if env::var_os(CHILD).is_none() {
        let name = format!(
            "{}::audit_error_diagnostics_stay_with_the_caller_and_out_of_tracing",
            module_path!().split_once("::").unwrap().1
        );
        let output = Command::new(env::current_exe().unwrap())
            .args(["--exact", &name, "--test-threads=1"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(stdout.contains("1 passed"), "{stdout}");
        return;
    }
    let captured = Warnings::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for error in [
                AgentError::Protocol(SENTINEL.into()),
                AgentError::Transport(SENTINEL.into()),
                AgentError::Configuration(SENTINEL.into()),
                AgentError::Provider {
                    code: 7,
                    diagnostic: Some(ProviderDiagnostic::new(SENTINEL)),
                },
                AgentError::AuthenticationRequired {
                    diagnostic: Some(ProviderDiagnostic::new(SENTINEL)),
                },
            ] {
                let agent = prepared(
                    Arc::new(InMemoryStorage::new()),
                    Arc::new(RefusingAudit(error.clone())),
                )
                .await;
                assert_eq!(
                    agent
                        .record_scheduling_audit(test_admission(&agent), "queue_admission")
                        .await,
                    Err(AuditObservationFailure::Unacknowledged(error))
                );
                let logged =
                    String::from_utf8(std::mem::take(&mut *captured.0.lock().unwrap())).unwrap();
                assert!(logged.contains("audit observation failed"), "{logged}");
                assert!(logged.contains("operation=\"queue_admission\""), "{logged}");
                assert!(logged.contains("failure=\"unacknowledged\""), "{logged}");
                assert!(!logged.contains(SENTINEL), "{logged}");
            }
            let agent = prepared(Arc::new(InMemoryStorage::new()), Arc::new(PanickingAudit)).await;
            assert_eq!(
                agent
                    .record_scheduling_audit(test_admission(&agent), "queue_admission")
                    .await,
                Err(AuditObservationFailure::Panicked)
            );
            let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
            assert!(logged.contains("failure=\"panicked\""), "{logged}");
            assert!(!logged.contains(SENTINEL), "{logged}");
        });
}
