use crate::shell::application::*;
use crate::shell::domain::*;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::watch;
struct FakeRunner {
    calls: Mutex<Vec<String>>,
}
impl Runner for FakeRunner {
    fn run<'a>(
        &'a self,
        request: &'a RunRequest,
        _: watch::Receiver<Option<StopCause>>,
        _: &'a dyn Audit,
    ) -> Task<'a, RunResult> {
        self.calls.lock().unwrap().push(request.id.clone());
        Box::pin(async {
            RunResult {
                cause: RunCause::Exited,
                scope_id: Some(3),
                process_id: Some(5),
                os_pid: Some(7),
                exit_code: Some(0),
                exit_signal: None,
                forced: Some(false),
                stdout: "fake".into(),
                stderr: String::new(),
                dropped_bytes: 0,
                cleanup_verified: true,
                cleanup_error: None,
                output_errors: vec![],
                audit_error: None,
            }
        })
    }
}
struct FakeAudit {
    fail_at: usize,
    events: Mutex<Vec<(String, Evidence)>>,
}
impl Audit for FakeAudit {
    fn record<'a>(
        &'a self,
        request: &'a RunRequest,
        evidence: Evidence,
    ) -> Task<'a, Result<(), ()>> {
        let mut events = self.events.lock().unwrap();
        events.push((request.id.clone(), evidence));
        let failed = events.len() == self.fail_at;
        Box::pin(async move {
            if failed {
                Err(())
            } else {
                Ok(())
            }
        })
    }
}
fn request() -> RunRequest {
    RunRequest {
        id: "one".into(),
        invocation: ToolInvocation::provider("provider-connection", ToolRequestId::Unsigned(7))
            .unwrap(),
        command: ShellCommand::new("true".into(), 1).unwrap(),
        cwd: "/workspace".into(),
    }
}
#[tokio::test]
async fn independent_injected_instances_and_admission_audit_gate() {
    for fail_at in [0, 1, 2] {
        let runner = Arc::new(FakeRunner {
            calls: Mutex::new(vec![]),
        });
        let audit = Arc::new(FakeAudit {
            fail_at,
            events: Mutex::new(vec![]),
        });
        let service = ShellService::new(runner.clone(), audit.clone());
        let (_stop, receive) = watch::channel(None);
        let result = service.run(&request(), receive).await;
        if fail_at == 1 {
            assert!(matches!(result, Err(ShellError::AdmissionAuditFailed)));
            assert!(runner.calls.lock().unwrap().is_empty());
        } else {
            let result = result.unwrap();
            assert!(result.cleanup_verified);
            assert_eq!(result.exit_code, Some(0));
            assert_eq!(result.audit_error.is_some(), fail_at == 2);
            assert_eq!(*runner.calls.lock().unwrap(), ["one"]);
            let events = audit.events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert!(matches!(events[0].1, Evidence::Admitted));
            assert!(matches!(events[1].1, Evidence::Finished(_)));
        }
    }
}
