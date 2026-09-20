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

#[test]
fn a_close_that_failed_twice_is_named_for_the_conversation_that_did_not_close() {
    let cleanup = || ConversationError::AttachmentCleanup {
        storage_failures: 1,
        audit_failures: 2,
    };
    // Closed, but its uploads were not all let go: cleanup the caller can retry.
    for closed in [
        cleanup(),
        ConversationError::AttachmentRelease(Box::new(cleanup())),
    ] {
        assert_eq!(error_code(&closed), "attachment_cleanup_unavailable");
    }
    // Not closed and not cleaned up: what the caller must act on is the close,
    // and closing again lets go of the uploads again.
    for (agent, code) in [
        (ConversationError::Capacity, "conversation_capacity"),
        (
            ConversationError::Agent(AgentError::Deadline),
            "agent_operation_failed",
        ),
    ] {
        let both = ConversationError::CloseIncomplete {
            agent: Box::new(agent),
            release: Box::new(cleanup()),
        };
        assert_eq!(error_code(&both), code);
    }
}
