use super::super::audit_diagnostics::*;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn logged(operation: impl FnOnce()) -> String {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, operation);
    let bytes = captured.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn audit_io_diagnostics_preserve_os_cause_and_the_original_failure() {
    let text = logged(|| {
        let error = observe_io::<()>(
            "execution",
            "sync_file",
            Err(io::Error::from_raw_os_error(28)),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(28));
    });
    assert!(text.contains("audit_kind=\"execution\""), "{text}");
    assert!(text.contains("audit_phase=\"sync_file\""), "{text}");
    assert!(text.contains("os_error=Some(28)"), "{text}");
    assert!(text.contains("error_kind="), "{text}");
}

#[test]
fn audit_io_diagnostics_never_log_the_error_display_or_contents() {
    let secret = "secret-token-and-private-path";
    let text = logged(|| {
        let error = observe_io::<()>(
            "creation",
            "persist",
            Err(io::Error::new(io::ErrorKind::PermissionDenied, secret)),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), secret);
    });
    assert!(text.contains("audit_phase=\"persist\""), "{text}");
    assert!(text.contains("PermissionDenied"), "{text}");
    assert!(text.contains("os_error=None"), "{text}");
    assert!(!text.contains(secret), "{text}");
}

#[test]
fn successful_audit_io_is_unchanged_and_silent() {
    let text = logged(|| assert_eq!(observe_io("execution", "sync_file", Ok(7)).unwrap(), 7));
    assert!(text.is_empty(), "{text}");
}

#[test]
fn audit_worker_interruption_names_phase_without_fabricating_an_os_error() {
    let text = logged(|| unavailable("execution", "worker"));
    assert!(text.contains("audit_phase=\"worker\""), "{text}");
    assert!(!text.contains("os_error"), "{text}");
}

#[test]
fn failed_physical_execution_write_keeps_typed_refusal_and_safe_diagnostics() {
    use super::{ExecutionAuditFileWriter, PrivateExecutionAuditFileWriter};
    use nessa_sdk::application::agent_execution::agents::AgentError;
    let directory = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let path = directory
        .path()
        .join("secret-private-path")
        .join("record.json");
    let text = logged(|| {
        let result = PrivateExecutionAuditFileWriter.write(&path, b"secret-audit-payload");
        assert!(matches!(result, Err(AgentError::AuditFailure)));
    });
    assert!(text.contains("audit_phase=\"create_temp\""), "{text}");
    assert!(!text.contains("secret-private-path"), "{text}");
    assert!(!text.contains("secret-audit-payload"), "{text}");
}

#[test]
fn serde_writer_error_retains_the_raw_os_cause() {
    struct Failed;
    impl Write for Failed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from_raw_os_error(28))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let error =
        serde_json::to_writer(Failed, &serde_json::json!({"record": "secret"})).unwrap_err();
    assert!(error.is_io());
    let error: io::Error = error.into();
    assert_eq!(error.raw_os_error(), Some(28));
}
