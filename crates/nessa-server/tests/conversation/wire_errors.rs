//! Opening failures retain their actionable meaning at the product boundary.
use super::{
    error_code, permission_answer_failure, AgentError, ConversationError, OutgoingMessage,
    PermissionSelectionState, StorageError,
};

#[test]
fn configuration_mismatch_is_not_a_transient_outage() {
    for error in [
        ConversationError::Storage(StorageError::IdentityMismatch),
        ConversationError::Agent(AgentError::Storage(StorageError::IdentityMismatch)),
    ] {
        assert_eq!(error_code(&error), "conversation_configuration_changed");
    }
    assert_eq!(
        error_code(&ConversationError::Unavailable),
        "temporarily_unavailable"
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
        assert_eq!(error.code, "audit_unavailable");
        assert_eq!(
            error.details.expect("typed details")["selectionState"],
            expected
        );
    }
}
