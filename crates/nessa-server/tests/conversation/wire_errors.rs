//! Opening failures retain their actionable meaning at the product boundary.
use super::{
    error_code, permission_answer_failure, AgentError, ConversationError, ConversationErrorCode,
    ImageInputRefusal, OutgoingMessage, PermissionSelectionState, StorageError,
};
use nessa_sdk::{
    application::agent_execution::{
        agents::{AgentStartupContext, AgentStartupPhase, AgentStartupStep},
        providers::UserImageError,
    },
    domain::common::value_objects::ImageMediaType,
};

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
        AgentStartupPhase::Session,
        AgentStartupPhase::Configure,
    ] {
        for context in [AgentStartupContext::New, AgentStartupContext::Restored] {
            let code = error_code(&ConversationError::Agent(AgentError::StartupDeadline(
                AgentStartupStep::new(phase, context),
            )));
            assert_eq!(code, ConversationErrorCode::AgentStartupDeadline);
            assert_eq!(code.as_str(), "agent_startup_deadline");
        }
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
        assert_eq!(
            error_code(&closed),
            ConversationErrorCode::AttachmentCleanupUnavailable
        );
    }
    // Not closed and not cleaned up: what the caller must act on is the close,
    // and closing again lets go of the uploads again.
    for (agent, code) in [
        (
            ConversationError::Capacity,
            ConversationErrorCode::ConversationCapacity,
        ),
        (
            ConversationError::Agent(AgentError::Deadline),
            ConversationErrorCode::AgentOperationFailed,
        ),
    ] {
        let both = ConversationError::CloseIncomplete {
            agent: Box::new(agent),
            release: Box::new(cleanup()),
        };
        assert_eq!(error_code(&both), code);
    }
}

#[test]
fn an_image_message_refused_before_acceptance_says_which_kind_of_refusal_it_was() {
    // Every one of these is decided before the message is accepted, and the
    // client recovers the draft for exactly these codes. `agent_operation_failed`
    // would tell it delivery is unknown.
    for (error, code) in [
        (
            AgentError::ImageInputRefused(ImageInputRefusal::AgentDoesNotAccept),
            ConversationErrorCode::ImageInputUnsupported,
        ),
        (
            AgentError::ImageInputRefused(ImageInputRefusal::NotOffered),
            ConversationErrorCode::ImageInputUnsupported,
        ),
        (
            AgentError::ImageInputRefused(ImageInputRefusal::MediaType(ImageMediaType::Webp)),
            ConversationErrorCode::InvalidRequest,
        ),
        (
            AgentError::ImageInputRefused(ImageInputRefusal::ImageTooLarge {
                size: 9,
                max_bytes: 6,
            }),
            ConversationErrorCode::InvalidRequest,
        ),
        (
            AgentError::MessageTooLarge {
                encoded_bytes: 17,
                max_bytes: 16,
            },
            ConversationErrorCode::InvalidRequest,
        ),
        (
            AgentError::UserImage(UserImageError::Missing),
            ConversationErrorCode::AttachmentUnavailable,
        ),
    ] {
        assert_eq!(error_code(&ConversationError::Agent(error)), code);
    }
    assert_eq!(
        error_code(&ConversationError::ImagesUnsupported),
        ConversationErrorCode::ImageInputUnsupported
    );
}
