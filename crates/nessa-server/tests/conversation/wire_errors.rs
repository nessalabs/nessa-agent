//! A refused permission answer keeps what became of the selection apart from
//! its diagnostic code at the product boundary. The codes themselves are
//! `conversation/application/error_code.rs`'s, tested beside it.
use super::{
    permission_answer_failure, AgentError, ConversationErrorCode, OutgoingMessage,
    PermissionSelectionState,
};

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
