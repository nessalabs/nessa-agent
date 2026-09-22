//! Authenticated wire commands translate into the server-owned conversation service.
//! The socket has already checked current access and the conversation action grant.
use super::{
    generated::{
        ConversationAnswerParams, ConversationAnswerQuestionParams, ConversationCancelParams,
        ConversationCloseParams, ConversationCreateParams, ConversationCreateResult,
        ConversationErrorCode, ConversationMutationResult,
        ConversationPermissionAnswerErrorDetails, ConversationPermissionSelectionState,
        ConversationReadParams, ConversationRemoveParams, ConversationReorderParams,
        ConversationSendParams,
    },
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::{
    agents::domain::AgentId,
    conversation::{
        application::{
            ConversationCaller, ConversationError, QuestionChoiceInput, RequestedAgent,
            SubmissionMode, SubmittedFile, SubmittedImage, SubmittedMessage,
        },
        domain::ConversationId,
    },
    protocol::{OutgoingMessage, RequestFrame},
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_sdk::application::agent_execution::{
    agents::AgentError, permissions::PermissionSelectionState, providers::ImageInputRefusal,
    sessions::StorageError,
};

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    // This build runs no conversations at all, which is not the same fact as a
    // caller naming an agent this one is not configured for. Sharing a code
    // between them made the panel tell someone with a working Claude that the
    // gateway has no agent configured.
    let Some(service) = state.conversations.as_ref() else {
        // Not `agent_not_configured`: this gateway runs no conversations at
        // all, and telling a user with a working agent to go configure one
        // sends them to change something that was never the problem.
        return failure(
            &frame.id,
            ConversationErrorCode::ConversationsNotConfigured.as_str(),
        );
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
                        requested_agent(params.agent.as_deref()),
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
                        SubmittedMessage {
                            text: params.text,
                            images: params
                                .attachments
                                .into_iter()
                                .map(|image| SubmittedImage {
                                    digest: image.digest,
                                    media_type: image.mime_type,
                                    size: image.size,
                                })
                                .collect(),
                            files: params
                                .files
                                .into_iter()
                                .map(|file| SubmittedFile { path: file.path })
                                .collect(),
                        },
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
            "conversation.answerQuestion" => {
                let params = params!(ConversationAnswerQuestionParams);
                service
                    .answer_question(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        params.execution_id,
                        params.question_id,
                        params.choices.map(|choices| {
                            choices
                                .into_iter()
                                .map(|choice| QuestionChoiceInput {
                                    key: choice.key,
                                    values: choice.values,
                                    own_words: choice.own_words,
                                })
                                .collect()
                        }),
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
            _ => Ok(failure(
                &frame.id,
                ConversationErrorCode::UnknownMethod.as_str(),
            )),
        }
    }
    .await;
    match result {
        Ok(response) => response,
        Err(ConversationError::PermissionAnswer { error, selection }) => {
            permission_answer_failure(&frame.id, error, selection)
        }
        Err(error) => failure(&frame.id, error_code(&error).as_str()),
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
        error_code(&ConversationError::Agent(error)).as_str(),
        serde_json::to_value(details).expect("generated error details serialize"),
    )
}
fn error_code(error: &ConversationError) -> ConversationErrorCode {
    match error {
        ConversationError::InvalidInput => ConversationErrorCode::InvalidRequest,
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
        ConversationError::AgentNotConfigured => ConversationErrorCode::AgentNotConfigured,
        ConversationError::AgentUnsupported => ConversationErrorCode::AgentUnsupported,
        ConversationError::Capacity => ConversationErrorCode::ConversationCapacity,
        ConversationError::Unavailable
        | ConversationError::Retirement(_)
        | ConversationError::RetirementAdmission { .. } => {
            ConversationErrorCode::TemporarilyUnavailable
        }
        ConversationError::Storage(StorageError::IdentityMismatch)
        | ConversationError::Agent(AgentError::Storage(StorageError::IdentityMismatch)) => {
            ConversationErrorCode::ConversationConfigurationChanged
        }
        ConversationError::Audit => ConversationErrorCode::AuditUnavailable,
        ConversationError::Metadata | ConversationError::Storage(_) => {
            ConversationErrorCode::ConversationStorageUnavailable
        }
        ConversationError::Agent(error) => match error {
            AgentError::SubmissionConflict => ConversationErrorCode::SubmissionConflict,
            AgentError::SubmissionUnresolved => ConversationErrorCode::SubmissionUnresolved,
            AgentError::Closed => ConversationErrorCode::ConversationClosed,
            AgentError::StalePermission => ConversationErrorCode::StalePermission,
            AgentError::InvalidInput(_) => ConversationErrorCode::InvalidRequest,
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
            // Startup never reaches the provider with input, and the runtime is
            // warm afterwards: the same command is safe to send again.
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
    }
}

fn conversation_id(value: &str) -> Result<ConversationId, ConversationError> {
    ConversationId::new(value).map_err(|_| ConversationError::InvalidInput)
}

/// The agent a creation names, if it names one.
///
/// A name no adapter exists for is carried inward rather than refused here. It
/// is still refused — a misspelling is not the installation fact the service's
/// "not configured for that agent" states, so it keeps its own code — but only
/// at the point the name would be used, which is never for a conversation that
/// already exists. The wire says as much: `agent` is "Ignored when the
/// conversation already exists". Refusing it here made one stale remembered
/// name fail every send, close, reorder and permission answer in every
/// conversation on the machine, because the panel sends it on all of them.
fn requested_agent(value: Option<&str>) -> Option<RequestedAgent> {
    value.map(|name| AgentId::parse(name).map_or(RequestedAgent::Unknown, RequestedAgent::Known))
}

#[cfg(test)]
#[path = "../../tests/conversation/wire_errors.rs"]
mod tests;
