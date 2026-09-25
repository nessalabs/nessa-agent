//! Opening failures retain their actionable meaning at the product boundary.
use super::{
    error_code, permission_answer_failure, AgentError, ConversationError, ConversationErrorCode,
    DeletionFailures, ImageInputRefusal, OutgoingMessage, PermissionSelectionState, StorageError,
};
use nessa_sdk::{
    application::agent_execution::{
        agents::{
            AgentStartupContext, AgentStartupPhase, AgentStartupStep, AttachmentFailureCode,
            AttachmentPhase,
        },
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
fn attachment_readiness_and_stale_internal_authority_have_stable_codes() {
    for phase in [
        AttachmentPhase::Waiting,
        AttachmentPhase::Starting,
        AttachmentPhase::Attached,
    ] {
        assert_eq!(
            error_code(&ConversationError::Agent(
                AgentError::AttachmentUnavailable(phase)
            )),
            ConversationErrorCode::TemporarilyUnavailable
        );
    }
    for error in [
        AgentError::AttachmentUnavailable(AttachmentPhase::Absent),
        AgentError::AttachmentUnavailable(AttachmentPhase::Failed(AttachmentFailureCode::Provider)),
        AgentError::AttachmentAuthorizationStale,
    ] {
        assert_eq!(
            error_code(&ConversationError::Agent(error)),
            ConversationErrorCode::AgentOperationFailed
        );
    }
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
        ConversationError::Deleted,
    ] {
        let code = error_code(&error).as_str();
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

/// Saved state this gateway cannot read, all the way from the decode that
/// refused it to the code the panel is handed — the chain nobody had written
/// down, and the one where two layers used to disagree.
///
/// The gateway decides once that trying again cannot differ, caches that, and
/// answers every later command from it without going near the provider. That
/// fact has to survive into the code it sends, or the panel offers a refresh
/// that can only ever return this, forever, which is what
/// `agent_operation_failed` produced: the panel reads it as `unavailable`,
/// whose sentence promises it keeps trying.
#[test]
fn state_this_gateway_cannot_read_is_permanent_and_says_so_on_the_wire() {
    {
        let failure = AgentError::Storage(StorageError::Corrupt(
            "a saved message names a path no domain would accept".into(),
        ));
        // The wire says which kind of permanence it is, rather than the
        // catch-all that says nothing about whether a retry could differ.
        // That the gateway decides once and caches it is `retryable_agent_open`
        // in the service, which these two are named in.
        assert_eq!(
            error_code(&ConversationError::Agent(failure)),
            ConversationErrorCode::ConversationStateUnreadable
        );
    }

    // An identity mismatch is not this: it means the configuration changed,
    // which is answered as that and already tells somebody to start a new
    // conversation. Two permanent things, told apart by which one happened.
    assert_eq!(
        error_code(&ConversationError::Agent(AgentError::Storage(
            StorageError::IdentityMismatch
        ))),
        ConversationErrorCode::ConversationConfigurationChanged
    );

    // Storage failures that are *not* about unreadable state keep the general
    // code, because trying them again genuinely can differ: a lease somebody
    // else holds is given up, and an I/O failure can clear.
    for transient in [
        StorageError::Busy,
        StorageError::Io("the disk is full".into()),
    ] {
        assert_eq!(
            error_code(&ConversationError::Agent(AgentError::Storage(transient))),
            ConversationErrorCode::AgentOperationFailed
        );
    }
}

#[test]
fn a_deleted_conversation_and_an_unfinished_deletion_have_their_own_codes() {
    assert_eq!(
        error_code(&ConversationError::Deleted),
        ConversationErrorCode::ConversationDeleted
    );
    let unfinished = |failures: DeletionFailures| {
        error_code(&ConversationError::DeletionIncomplete(Box::new(failures)))
    };
    let lost_evidence = || ConversationError::AttachmentCleanup {
        storage_failures: 0,
        audit_failures: 1,
    };
    let kept_files = || ConversationError::AttachmentCleanup {
        storage_failures: 1,
        audit_failures: 0,
    };
    // Something is left to erase: the conversation is deleted, and repeating
    // the delete finishes it.
    for failures in [
        DeletionFailures {
            stop: Some(crate::conversation::application::StopFailure::Failed(
                AgentError::CleanupUncertain,
            )),
            ..DeletionFailures::default()
        },
        DeletionFailures {
            history: Some(ConversationError::Storage(StorageError::Busy)),
            ..DeletionFailures::default()
        },
        DeletionFailures {
            history_held: true,
            ..DeletionFailures::default()
        },
        DeletionFailures {
            history_held: true,
            attachments: Some(lost_evidence()),
            ..DeletionFailures::default()
        },
        DeletionFailures {
            summary: Some(ConversationError::Metadata),
            ..DeletionFailures::default()
        },
        DeletionFailures {
            attachments: Some(kept_files()),
            ..DeletionFailures::default()
        },
        // Evidence lost for uploads does not hide history left behind.
        DeletionFailures {
            history: Some(ConversationError::Storage(StorageError::Busy)),
            attachments: Some(lost_evidence()),
            ..DeletionFailures::default()
        },
        // The agent could not be asked about its own record, or refused;
        // with uploads' evidence lost beside it, that is still what is left.
        DeletionFailures {
            provider: Some(ConversationError::Agent(AgentError::Deadline)),
            ..DeletionFailures::default()
        },
        DeletionFailures {
            provider: Some(ConversationError::Agent(AgentError::Deadline)),
            attachments: Some(lost_evidence()),
            ..DeletionFailures::default()
        },
        // Another attempt left it unfinished; this request did not try again.
        DeletionFailures {
            another_attempt: true,
            ..DeletionFailures::default()
        },
        // A step past the fence fell over, beside uploads' evidence lost: what
        // is left is whatever the tombstone says.
        DeletionFailures {
            interrupted: true,
            attachments: Some(lost_evidence()),
            ..DeletionFailures::default()
        },
        // How far it got was not written down, so the next try repeats it.
        DeletionFailures {
            tombstone: Some(ConversationError::Metadata),
            attachments: Some(lost_evidence()),
            ..DeletionFailures::default()
        },
    ] {
        assert_eq!(
            unfinished(failures.clone()),
            ConversationErrorCode::ConversationErasureIncomplete,
            "{failures:?}"
        );
    }
    // The deletion record was not taken, or only uploads' records were lost:
    // the answer is the lost record, whatever else is also true.
    for failures in [
        DeletionFailures {
            audit: Some(ConversationError::Audit),
            ..DeletionFailures::default()
        },
        DeletionFailures {
            audit: Some(ConversationError::Audit),
            attachments: Some(kept_files()),
            ..DeletionFailures::default()
        },
        DeletionFailures {
            attachments: Some(lost_evidence()),
            ..DeletionFailures::default()
        },
    ] {
        assert_eq!(
            unfinished(failures.clone()),
            ConversationErrorCode::AuditUnavailable,
            "{failures:?}"
        );
    }
}
