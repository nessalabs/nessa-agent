//! Authenticated wire commands translate into the server-owned conversation service.
//! The socket has already checked current access and the conversation action grant.
use super::{
    generated::{
        ApprovalMode as WireApprovalMode, ConversationAnswerParams,
        ConversationAnswerQuestionParams, ConversationArchiveParams, ConversationCancelParams,
        ConversationCloseParams, ConversationCreateParams, ConversationCreateResult,
        ConversationDeleteParams, ConversationListParams, ConversationListResult,
        ConversationMutationResult, ConversationPermissionAnswerErrorDetails,
        ConversationPermissionSelectionState, ConversationReadParams, ConversationRemoveParams,
        ConversationReorderParams, ConversationSendParams, ConversationSetApprovalModeParams,
        ConversationSetApprovalModeResult, ConversationSummary,
        ConversationView as WireConversationView,
    },
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::{
    agents::domain::AgentId,
    conversation::{
        application::{
            error_code, ConversationCaller, ConversationError, ConversationList,
            ConversationView as ApplicationConversationView, QuestionChoiceInput, RequestedAgent,
            RequestedConversation, SubmissionMode, SubmittedFile, SubmittedImage, SubmittedMessage,
        },
        domain::{ConversationApprovalMode, ConversationId},
    },
    product_contract::generated::ConversationErrorCode,
    protocol::{OutgoingMessage, RequestFrame},
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_sdk::application::agent_execution::{
    agents::AgentError, permissions::PermissionSelectionState,
};
use serde_json::{json, Value};

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
    let caller = |request_id: String| caller(session, request_id);
    let result: Result<OutgoingMessage, ConversationError> = async {
        match frame.method.as_str() {
            "conversation.create" => {
                let params = params!(ConversationCreateParams);
                service
                    .create(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id),
                        RequestedConversation {
                            agent: requested_agent(params.agent.as_deref()),
                            model: params.model,
                            approval_mode: params.approval_mode.map(|mode| match mode {
                                WireApprovalMode::Ask => ConversationApprovalMode::Ask,
                                WireApprovalMode::Auto => ConversationApprovalMode::Auto,
                                WireApprovalMode::Full => ConversationApprovalMode::Full,
                            }),
                        },
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
                Ok(success(&frame.id, &wire_view(state, view)?))
            }
            "conversation.setApprovalMode" => {
                let params = params!(ConversationSetApprovalModeParams);
                let selected = service
                    .set_approval_mode(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        match params.mode {
                            WireApprovalMode::Ask => ConversationApprovalMode::Ask,
                            WireApprovalMode::Auto => ConversationApprovalMode::Auto,
                            WireApprovalMode::Full => ConversationApprovalMode::Full,
                        },
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationSetApprovalModeResult {
                        request_id: params.request_id,
                        mode: match selected {
                            ConversationApprovalMode::Ask => WireApprovalMode::Ask,
                            ConversationApprovalMode::Auto => WireApprovalMode::Auto,
                            ConversationApprovalMode::Full => WireApprovalMode::Full,
                        },
                    },
                ))
            }
            "conversation.list" => {
                let ConversationListParams { archived } = params!(ConversationListParams);
                let listed = service
                    .list(caller(frame.id.clone()), archived.unwrap_or(false))
                    .await?;
                Ok(success(&frame.id, &list_result(listed)))
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
            "conversation.archive" | "conversation.unarchive" => {
                let params = params!(ConversationArchiveParams);
                let applied = service
                    .archive(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        frame.method == "conversation.archive",
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
            "conversation.delete" => {
                let params = params!(ConversationDeleteParams);
                let applied = service
                    .delete(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
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

/// Who sends a conversation command: the credential's verified identity.
/// The caller's optional client metadata is deliberately not used to
/// attribute SDK commands.
pub(super) fn caller(session: &AuthenticatedSession, request_id: String) -> ConversationCaller {
    let context = session.context();
    ConversationCaller {
        organization_id: context.organization_id().clone(),
        principal_id: context.principal_id().clone(),
        surface_id: context.credential_id().as_str().to_owned(),
        action_id: request_id,
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

pub(super) fn conversation_id(value: &str) -> Result<ConversationId, ConversationError> {
    ConversationId::new(value).map_err(|_| ConversationError::InvalidInput)
}

/// The authenticated catalog owns display facts and preset descriptions. The
/// application view owns the committed selection; the wire joins them once.
fn wire_view(
    state: &ProductRouteState,
    view: ApplicationConversationView,
) -> Result<WireConversationView, ConversationError> {
    let selection = view
        .selection
        .clone()
        .ok_or(ConversationError::Unavailable)?;
    let model = state
        .agents_catalog
        .as_ref()
        .and_then(|catalog| {
            catalog
                .agents
                .iter()
                .find(|agent| agent.agent == selection.agent.name())
        })
        .and_then(|agent| {
            agent
                .models
                .iter()
                .find(|model| model.model_id == selection.model)
        })
        .ok_or(ConversationError::ModelUnavailable)?;
    if !model
        .approval_modes
        .iter()
        .any(|mode| mode.id.as_str() == selection.approval_mode.as_str())
    {
        return Err(ConversationError::ApprovalModeUnavailable);
    }
    let mut value = serde_json::to_value(view).map_err(|_| ConversationError::Unavailable)?;
    let fields = value
        .as_object_mut()
        .ok_or(ConversationError::Unavailable)?;
    fields.insert(
        "approvalMode".into(),
        json!(selection.approval_mode.as_str()),
    );
    fields.insert(
        "approvalModes".into(),
        serde_json::to_value(&model.approval_modes).map_err(|_| ConversationError::Unavailable)?,
    );
    if let Some(runtime) = fields.get_mut("runtime") {
        let runtime = runtime
            .as_object_mut()
            .ok_or(ConversationError::Unavailable)?;
        runtime.insert("agent".into(), json!(selection.agent.name()));
        runtime.insert("modelName".into(), json!(model.display_name));
        runtime.insert(
            "contextWindowTokens".into(),
            json!(model.max_context_window_tokens),
        );
        runtime.insert("reasoning".into(), Value::Bool(model.reasoning));
    }
    serde_json::from_value(value).map_err(|_| ConversationError::Unavailable)
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

/// A list as the wire carries it, saying whether the bound left any out
/// (`a_cut_list_says_so_on_the_wire`).
fn list_result(listed: ConversationList) -> ConversationListResult {
    ConversationListResult {
        conversations: listed
            .conversations
            .into_iter()
            .map(|entry| ConversationSummary {
                conversation_id: entry.conversation_id,
                title: entry.title,
                preview: entry.preview,
                created_at_ms: entry.created_at_ms,
                updated_at_ms: entry.updated_at_ms,
                running: entry.running,
                archived: entry.archived,
            })
            .collect(),
        complete: listed.complete,
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/wire_errors.rs"]
mod tests;
#[cfg(test)]
#[path = "../../tests/conversation/wire_list.rs"]
mod wire_list;
