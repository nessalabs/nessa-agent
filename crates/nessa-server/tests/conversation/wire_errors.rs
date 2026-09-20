//! Opening failures retain their actionable meaning at the product boundary.
use super::{
    error_code, permission_answer_failure, AgentError, ConversationError, ConversationErrorCode,
    OutgoingMessage, PermissionSelectionState, StorageError,
};
use nessa_sdk::application::agent_execution::agents::AgentStartupPhase;

#[test]
fn configuration_mismatch_is_not_a_transient_outage() {
    for error in [
        ConversationError::Storage(StorageError::IdentityMismatch),
        ConversationError::Agent(AgentError::Storage(StorageError::IdentityMismatch)),
    ] {
        assert_eq!(
            error_code(&error),
            ConversationErrorCode::ConversationConfigurationChanged
        );
    }
    assert_eq!(
        error_code(&ConversationError::Unavailable),
        ConversationErrorCode::TemporarilyUnavailable
    );
}

#[test]
fn startup_deadline_is_distinguished_from_other_agent_failures() {
    for phase in [
        AgentStartupPhase::Initialize,
        AgentStartupPhase::SessionNew,
        AgentStartupPhase::SessionResume,
        AgentStartupPhase::SessionConfigure,
    ] {
        let code = error_code(&ConversationError::Agent(AgentError::StartupDeadline(
            phase,
        )));
        assert_eq!(code, ConversationErrorCode::AgentStartupDeadline);
        assert_eq!(code.as_str(), "agent_startup_deadline");
    }
    // A deadline outside startup keeps the unclassified code; it says nothing
    // about whether the command was admitted.
    assert_eq!(
        error_code(&ConversationError::Agent(AgentError::Deadline)),
        ConversationErrorCode::AgentOperationFailed
    );
}

#[test]
fn permission_answer_failure_preserves_selection_separately_from_diagnostic_code() {
    for (selection, expected) in [
        (PermissionSelectionState::Pending, "pending"),
        (PermissionSelectionState::Consumed, "consumed"),
        (PermissionSelectionState::Unknown, "unknown"),
    ] {
        let OutgoingMessage::Response(response) =
            permission_answer_failure("answer", AgentError::AuditFailure, selection)
        else {
            panic!("response expected")
        };
        let error = response.error.expect("error response");
        assert_eq!(error.code, ConversationErrorCode::AuditUnavailable.as_str());
        assert_eq!(
            error.details.expect("typed details")["selectionState"],
            expected
        );
    }
}
