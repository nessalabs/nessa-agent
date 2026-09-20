//! The gateway log must describe the opening that actually failed.
use super::{opening_failure_report, AgentError, StorageError};
use nessa_sdk::application::agent_execution::agents::AgentStartupPhase;

#[test]
fn a_startup_deadline_names_its_step_and_whether_context_was_restored() {
    for (phase, step, session) in [
        (AgentStartupPhase::Initialize, "initialize", "new"),
        (AgentStartupPhase::SessionNew, "session_new", "new"),
        (
            AgentStartupPhase::SessionResume,
            "session_resume",
            "restored",
        ),
        (
            AgentStartupPhase::SessionConfigure,
            "session_configure",
            "new",
        ),
    ] {
        let report = opening_failure_report(&AgentError::StartupDeadline(phase));
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
