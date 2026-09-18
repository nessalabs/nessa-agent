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

/// The other side of these codes is `REFUSALS` in
/// `packages/nessa-client/src/application/conversation-mutation-error.ts`, which
/// hardcodes each string and turns it into the sentence a person reads. They
/// cross a language boundary nothing else checks, so a rename on either side
/// fails here rather than at runtime — the same guard the readiness names have
/// in `tests/agents/http.rs`.
#[test]
fn every_refusal_this_gateway_states_is_understood_by_the_client() {
    let client = include_str!(
        "../../../../packages/nessa-client/src/application/conversation-mutation-error.ts"
    );
    for error in [
        ConversationError::AgentNotConfigured,
        ConversationError::AgentUnsupported,
    ] {
        let code = error_code(&error);
        assert!(
            client.contains(&format!("{code}:")),
            "the client does not recognize {code:?}"
        );
    }
    // `conversations_not_configured` is stated where there is no service to
    // fail, so it has no `ConversationError` to derive it from; it is pinned
    // end to end by the gateway tests instead.
    assert!(client.contains("conversations_not_configured:"));
}
