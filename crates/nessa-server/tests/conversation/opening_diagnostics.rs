//! The gateway log must describe the opening that actually failed.
use super::{
    opening_failure_report, report_opening_failure, AgentError, ConversationId, StorageError,
};
use nessa_sdk::application::agent_execution::agents::{
    AgentStartupContext, AgentStartupPhase, AgentStartupStep,
};
use std::sync::{Arc, Mutex, PoisonError};

#[test]
fn a_startup_deadline_names_its_step_and_whether_context_was_restored() {
    for (phase, context, step, session) in [
        (
            AgentStartupPhase::Initialize,
            AgentStartupContext::New,
            "initialize",
            "new",
        ),
        (
            AgentStartupPhase::Session,
            AgentStartupContext::New,
            "session_new",
            "new",
        ),
        (
            AgentStartupPhase::Configure,
            AgentStartupContext::New,
            "session_configure",
            "new",
        ),
        // A restoration can run out of budget before `session/resume` is sent,
        // and again while configuring the session it just resumed. Both are
        // restorations; reading that off the step would call them new.
        (
            AgentStartupPhase::Initialize,
            AgentStartupContext::Restored,
            "initialize",
            "restored",
        ),
        (
            AgentStartupPhase::Session,
            AgentStartupContext::Restored,
            "session_resume",
            "restored",
        ),
        (
            AgentStartupPhase::Configure,
            AgentStartupContext::Restored,
            "session_configure",
            "restored",
        ),
    ] {
        let report = opening_failure_report(&AgentError::StartupDeadline(AgentStartupStep::new(
            phase, context,
        )));
        assert_eq!(report.message, "agent startup exceeded its budget");
        assert_eq!(report.phase, Some(step));
        assert_eq!(report.session, Some(session));
    }
}

#[test]
fn other_opening_failures_do_not_claim_a_restoration_that_never_ran() {
    for error in [
        AgentError::Deadline,
        AgentError::Transport("pipe closed".into()),
        AgentError::Storage(StorageError::IdentityMismatch),
        AgentError::Configuration("bad executable".into()),
    ] {
        let report = opening_failure_report(&error);
        assert_eq!(report.message, "conversation agent opening failed");
        assert_eq!(report.phase, None);
        assert_eq!(report.session, None);
    }
}

/// The report exists to be logged, so the event itself is checked too: a field
/// dropped from the tracing call would otherwise pass every test above.
#[test]
fn the_logged_event_carries_the_step_and_context_it_reports() {
    let captured = Capture(Arc::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let id = ConversationId::new("504c3377-0000-4000-8000-000000000000").unwrap();
    tracing::subscriber::with_default(subscriber, || {
        report_opening_failure(
            &id,
            &AgentError::StartupDeadline(AgentStartupStep::new(
                AgentStartupPhase::Initialize,
                AgentStartupContext::Restored,
            )),
        );
        // A failure that names no step must not emit empty or invented fields.
        report_opening_failure(&id, &AgentError::Transport("pipe closed".into()));
    });
    let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let (deadline, other) = text
        .split_once("conversation agent opening failed")
        .expect("both events logged");
    assert!(
        deadline.contains("agent startup exceeded its budget"),
        "{deadline}"
    );
    assert!(
        deadline.contains("504c3377-0000-4000-8000-000000000000"),
        "{deadline}"
    );
    assert!(deadline.contains("phase=\"initialize\""), "{deadline}");
    assert!(deadline.contains("session=\"restored\""), "{deadline}");
    assert!(deadline.contains("StartupDeadline"), "{deadline}");
    assert!(!other.contains("phase="), "{other}");
    assert!(!other.contains("session="), "{other}");
}

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
