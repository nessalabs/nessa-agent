//! Authenticated wire commands translate into the server-owned conversation service.
//! The socket has already checked current access and the conversation action grant.
use super::{
    generated::{
        ConversationAnswerParams, ConversationCancelParams, ConversationCloseParams,
        ConversationCreateParams, ConversationCreateResult, ConversationMutationResult,
        ConversationPermissionAnswerErrorDetails, ConversationPermissionSelectionState,
        ConversationReadParams, ConversationRemoveParams, ConversationReorderParams,
        ConversationSendParams,
    },
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::{
    conversation::{
        application::{ConversationCaller, ConversationError, SubmissionMode, SubmittedImage},
        domain::ConversationId,
    },
    protocol::{OutgoingMessage, RequestFrame},
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_sdk::application::agent_execution::{
    agents::AgentError, permissions::PermissionSelectionState, sessions::StorageError,
};

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(service) = state.conversations.as_ref() else {
        return failure(&frame.id, "agent_not_configured");
    };
    macro_rules! params {
        ($kind:ty) => {
            match serde_json::from_value::<$kind>(frame.params) {
                Ok(value) => value,
                Err(_) => return Err(ConversationError::InvalidInput),
            }
        };
    }
    let context = session.context();
    // Credential identity is a verified surface association. The caller's optional
    // client metadata is deliberately not used to attribute SDK commands.
    let caller = |request_id: String| ConversationCaller {
        organization_id: context.organization_id().clone(),
        principal_id: context.principal_id().clone(),
        surface_id: context.credential_id().as_str().to_owned(),
        action_id: request_id,
    };
    let result: Result<OutgoingMessage, ConversationError> = async {
        match frame.method.as_str() {
            "conversation.create" => {
                let params = params!(ConversationCreateParams);
                service
                    .create(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id),
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationCreateResult {
                        conversation_id: params.conversation_id,
                    },
                ))
            }
            "conversation.read" => {
                let params = params!(ConversationReadParams);
                let view = service
                    .read(
                        conversation_id(&params.conversation_id)?,
                        caller(frame.id.clone()),
                    )
                    .await?;
                Ok(success(&frame.id, &view))
            }
            "conversation.send" | "conversation.steer" => {
                let params = params!(ConversationSendParams);
                let mode = if frame.method == "conversation.steer" {
                    SubmissionMode::Steer
                } else {
                    SubmissionMode::Queue
                };
                let receipt = service
                    .submit(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id),
                        params.execution_id,
                        params.text,
                        params
                            .attachments
                            .into_iter()
                            .map(|image| SubmittedImage {
                                digest: image.digest,
                                media_type: image.mime_type,
                                size: image.size,
                            })
                            .collect(),
                        mode,
                    )
                    .await?;
                Ok(success(&frame.id, &receipt))
            }
            "conversation.reorder" => {
                let params = params!(ConversationReorderParams);
                let outcome = service
                    .reorder(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        params.execution_ids,
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &serde_json::json!({"requestId": params.request_id, "outcome": outcome}),
                ))
            }
            "conversation.remove" => {
                let params = params!(ConversationRemoveParams);
                let applied = service
                    .remove(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        params.execution_id,
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationMutationResult {
                        request_id: params.request_id,
                        applied,
                    },
                ))
            }
            "conversation.answer" => {
                let params = params!(ConversationAnswerParams);
                service
                    .answer(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        params.execution_id,
                        params.permission_id,
                        params.option_id,
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationMutationResult {
                        request_id: params.request_id,
                        applied: true,
                    },
                ))
            }
            "conversation.cancel" => {
                let params = params!(ConversationCancelParams);
                service
                    .cancel_permission(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        params.execution_id,
                        params.permission_id,
                        params.reason,
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationMutationResult {
                        request_id: params.request_id,
                        applied: true,
                    },
                ))
            }
            "conversation.close" => {
                let params = params!(ConversationCloseParams);
                service
                    .close(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationMutationResult {
                        request_id: params.request_id,
                        applied: true,
                    },
                ))
            }
            _ => Ok(failure(&frame.id, "unknown_method")),
        }
    }
    .await;
    match result {
        Ok(response) => response,
        Err(ConversationError::PermissionAnswer { error, selection }) => {
            permission_answer_failure(&frame.id, error, selection)
        }
        Err(error) => failure(&frame.id, error_code(&error)),
    }
}

fn permission_answer_failure(
    request_id: &str,
    error: AgentError,
    selection: PermissionSelectionState,
) -> OutgoingMessage {
    let selection_state = match selection {
        PermissionSelectionState::Pending => ConversationPermissionSelectionState::Pending,
        PermissionSelectionState::Consumed => ConversationPermissionSelectionState::Consumed,
        PermissionSelectionState::Unknown => ConversationPermissionSelectionState::Unknown,
    };
    let details = ConversationPermissionAnswerErrorDetails { selection_state };
    failure_with_details(
        request_id,
        error_code(&ConversationError::Agent(error)),
        serde_json::to_value(details).expect("generated error details serialize"),
    )
}
fn error_code(error: &ConversationError) -> &'static str {
    match error {
        ConversationError::InvalidInput => "invalid_request",
        ConversationError::ImagesUnsupported => "image_input_unsupported",
        ConversationError::AttachmentNotFound => "attachment_not_found",
        // The conversation did close; what failed is cleanup the caller can retry.
        ConversationError::AttachmentRelease(_) | ConversationError::AttachmentCleanup { .. } => {
            "attachment_cleanup_unavailable"
        }
        // The conversation did not close, which is what a caller must act on;
        // closing again also lets go of the uploads again. The release failure
        // stays in the typed error and the log, not in a second wire code.
        ConversationError::CloseIncomplete { agent, .. } => error_code(agent),
        ConversationError::NotFound => "conversation_not_found",
        ConversationError::Capacity => "conversation_capacity",
        ConversationError::Unavailable
        | ConversationError::Retirement(_)
        | ConversationError::RetirementAdmission { .. } => "temporarily_unavailable",
        ConversationError::Storage(StorageError::IdentityMismatch)
        | ConversationError::Agent(AgentError::Storage(StorageError::IdentityMismatch)) => {
            "conversation_configuration_changed"
        }
        ConversationError::Audit => "audit_unavailable",
        ConversationError::Metadata | ConversationError::Storage(_) => {
            "conversation_storage_unavailable"
        }
        ConversationError::Agent(error) => match error {
            AgentError::SubmissionConflict => "submission_conflict",
            AgentError::SubmissionUnresolved => "submission_unresolved",
            AgentError::Closed => "conversation_closed",
            AgentError::StalePermission => "stale_permission",
            AgentError::InvalidInput(_) => "invalid_request",
            AgentError::UserImage(_) => "attachment_unavailable",
            AgentError::AuditFailure => "audit_unavailable",
            _ => "agent_operation_failed",
        },
        ConversationError::PermissionAnswer { error, .. } => {
            error_code(&ConversationError::Agent(error.clone()))
        }
    }
}

fn conversation_id(value: &str) -> Result<ConversationId, ConversationError> {
    ConversationId::new(value).map_err(|_| ConversationError::InvalidInput)
}

#[cfg(test)]
#[path = "../../tests/conversation/wire_errors.rs"]
mod tests;
