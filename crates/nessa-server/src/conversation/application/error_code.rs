//! The protocol code each conversation error is answered with: the one
//! mapping, read by the wire when it answers and by the audit when it records
//! what an app's message was answered with (#390). The codes are the product
//! contract's, generated from the schema.
use super::{ConversationError, DeletionFailures};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_sdk::application::agent_execution::{
    agents::{AgentError, AttachmentPhase},
    providers::ImageInputRefusal,
    sessions::StorageError,
};
use nessa_sdk::domain::agent_execution::leases::LeaseRefusal;

pub fn error_code(error: &ConversationError) -> ConversationErrorCode {
    match error {
        ConversationError::InvalidInput | ConversationError::CatalogueInvalidRequest => {
            ConversationErrorCode::InvalidRequest
        }
        ConversationError::ImagesUnsupported => ConversationErrorCode::ImageInputUnsupported,
        ConversationError::AttachmentNotFound => ConversationErrorCode::AttachmentNotFound,
        // Everything was let go and only the evidence of it was lost, so there
        // is no cleanup left to retry: the answer is the lost record.
        ConversationError::AttachmentCleanup {
            storage_failures: 0,
            ..
        } => ConversationErrorCode::AuditUnavailable,
        // The conversation did close; what failed is cleanup the caller can retry.
        ConversationError::AttachmentRelease(_) | ConversationError::AttachmentCleanup { .. } => {
            ConversationErrorCode::AttachmentCleanupUnavailable
        }
        // The conversation did not close, which is what a caller must act on;
        // closing again also lets go of the uploads again. The release failure
        // stays in the typed error and the log, not in a second wire code.
        ConversationError::CloseIncomplete { agent, .. } => error_code(agent),
        ConversationError::NotFound => ConversationErrorCode::ConversationNotFound,
        ConversationError::Deleted => ConversationErrorCode::ConversationDeleted,
        ConversationError::DeletionIncomplete(failures) => deletion_incomplete(failures),
        ConversationError::AgentNotConfigured => ConversationErrorCode::AgentNotConfigured,
        ConversationError::AgentUnsupported => ConversationErrorCode::AgentUnsupported,
        ConversationError::LeaseRefused(LeaseRefusal::SandboxUnavailable) => {
            ConversationErrorCode::SandboxUnavailable
        }
        ConversationError::ModelUnavailable => ConversationErrorCode::ModelUnavailable,
        ConversationError::ApprovalModeUnavailable => {
            ConversationErrorCode::ApprovalModeUnavailable
        }
        ConversationError::RequestConflict => ConversationErrorCode::ApprovalRequestConflict,
        ConversationError::TurnRunning => ConversationErrorCode::TurnRunning,
        ConversationError::ApprovalModeNotApplied => ConversationErrorCode::ApprovalModeNotApplied,
        ConversationError::ApprovalModeUncertain => ConversationErrorCode::ApprovalModeUncertain,
        ConversationError::Capacity => ConversationErrorCode::ConversationCapacity,
        ConversationError::Unavailable
        | ConversationError::Retirement(_)
        | ConversationError::RetirementAdmission { .. } => {
            ConversationErrorCode::TemporarilyUnavailable
        }
        ConversationError::CatalogueIdentityChanged
        | ConversationError::Storage(StorageError::IdentityMismatch)
        | ConversationError::Agent(AgentError::Storage(StorageError::IdentityMismatch)) => {
            ConversationErrorCode::ConversationConfigurationChanged
        }
        ConversationError::Audit => ConversationErrorCode::AuditUnavailable,
        ConversationError::AdmissionEvidence { audit: Some(_), .. } => {
            ConversationErrorCode::AuditUnavailable
        }
        ConversationError::AdmissionEvidence {
            audit: None,
            storage: Some(_),
        } => ConversationErrorCode::ConversationStorageUnavailable,
        ConversationError::AdmissionEvidence {
            audit: None,
            storage: None,
        } => ConversationErrorCode::AgentOperationFailed,
        ConversationError::Metadata | ConversationError::Storage(_) => {
            ConversationErrorCode::ConversationStorageUnavailable
        }
        ConversationError::Agent(error) => match error {
            AgentError::AttachmentUnavailable(
                AttachmentPhase::Waiting | AttachmentPhase::Starting | AttachmentPhase::Attached,
            ) => ConversationErrorCode::TemporarilyUnavailable,
            AgentError::AttachmentUnavailable(
                AttachmentPhase::Absent | AttachmentPhase::Failed(_),
            )
            | AgentError::AttachmentAuthorizationStale => {
                ConversationErrorCode::AgentOperationFailed
            }
            AgentError::SubmissionConflict => ConversationErrorCode::SubmissionConflict,
            AgentError::SubmissionUnresolved => ConversationErrorCode::SubmissionUnresolved,
            AgentError::Closed => ConversationErrorCode::ConversationClosed,
            AgentError::StalePermission => ConversationErrorCode::StalePermission,
            // A message naming an app no earlier MCP tool call drew is a
            // request no conversation could take as it stands.
            AgentError::InvalidInput(_) | AgentError::UnknownApp(_) => {
                ConversationErrorCode::InvalidRequest
            }
            // Refused before the message was accepted, so the caller still has it.
            // An agent that takes no images is the same fact whether this service
            // or the SDK's admission noticed it. An image outside the model's
            // limits, or a message too large for one frame, is a request this
            // gateway could never have delivered as it stands.
            // No images here, whichever layer noticed: the agent declined them,
            // or this model or binding offers none. One fact, one code.
            AgentError::ImageInputRefused(
                ImageInputRefusal::AgentDoesNotAccept | ImageInputRefusal::NotOffered,
            ) => ConversationErrorCode::ImageInputUnsupported,
            AgentError::ImageInputRefused(
                ImageInputRefusal::MediaType(_) | ImageInputRefusal::ImageTooLarge { .. },
            )
            | AgentError::MessageTooLarge { .. } => ConversationErrorCode::InvalidRequest,
            AgentError::UserImage(_) => ConversationErrorCode::AttachmentUnavailable,
            AgentError::AuditFailure => ConversationErrorCode::AuditUnavailable,
            // Startup never reaches the provider with input. This code carries
            // no claim that a later attachment attempt will succeed.
            AgentError::StartupDeadline(_) => ConversationErrorCode::AgentStartupDeadline,
            // Saved state this gateway cannot read, which it will not be able
            // to read later either: the failure is cached and every later
            // command is answered from it without touching the provider again.
            // Its own code, because `agent_operation_failed` says nothing about
            // whether trying again could differ, and a panel that assumes it
            // could offers a retry that can only ever return this.
            // `IdentityMismatch` is answered above as a changed configuration,
            // which is what it means and already says "start a new one".
            AgentError::Storage(StorageError::Corrupt(_)) => {
                ConversationErrorCode::ConversationStateUnreadable
            }
            _ => ConversationErrorCode::AgentOperationFailed,
        },
        ConversationError::PermissionAnswer { error, .. } => {
            error_code(&ConversationError::Agent(error.clone()))
        }
        // Audit already named this with the protocol's code.
        ConversationError::McpApp(error) => error.code(),
    }
}

/// A delete that happened and did not finish. What is left to erase is what a
/// caller acts on — repeating the delete finishes it — unless the one thing
/// missing is evidence: a deletion record the sink did not take, which kept
/// the history too, or uploads let go without their records, where nothing is
/// left to erase and the answer is the lost record, as it is for `close`.
fn deletion_incomplete(failures: &DeletionFailures) -> ConversationErrorCode {
    // Every field named, so a new one cannot be left out of this decision
    // without failing to compile.
    let DeletionFailures {
        stop,
        history,
        history_leased_elsewhere,
        provider,
        no_agent_slot,
        audit,
        attachments,
        summary,
        tombstone,
        interrupted,
        another_attempt,
    } = failures;
    let evidence_only = stop.is_none()
        && history.is_none()
        && !history_leased_elsewhere
        && provider.is_none()
        && !no_agent_slot
        && summary.is_none()
        && tombstone.is_none()
        && !interrupted
        && !another_attempt
        && matches!(
            attachments,
            None | Some(ConversationError::AttachmentCleanup {
                storage_failures: 0,
                ..
            })
        );
    if audit.is_some() || (evidence_only && attachments.is_some()) {
        ConversationErrorCode::AuditUnavailable
    } else {
        ConversationErrorCode::ConversationErasureIncomplete
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/error_code.rs"]
mod tests;
