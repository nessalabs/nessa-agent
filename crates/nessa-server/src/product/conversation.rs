//! Authenticated wire commands translate into the server-owned conversation service.
//! The socket has already checked current access and the conversation action grant.
use super::{
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::conversation::application::{
    error_code, ConversationCaller, ConversationError, ConversationService, QuestionChoiceInput, RequestedAgent,
    RequestedConversation, SubmissionMode, SubmittedFile, SubmittedImage, SubmittedMessage,
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::conversation::projection::CommittedCursor;
use nessa_protocol::conversation::view::{
    ConversationList, ConversationObservation, ConversationObservationCursor,
    ConversationView as ApplicationConversationView,
};
use nessa_protocol::product::generated::{
    ApprovalMode as WireApprovalMode, ConversationAnswerParams, ConversationAnswerQuestionParams,
    ConversationArchiveParams, ConversationCancelParams, ConversationCloseParams,
    ConversationCommandOperation, ConversationCommandOutcome, ConversationCommandReceipt,
    ConversationCommandStage, ConversationCreateParams, ConversationCreateResult,
    ConversationDeleteParams, ConversationListParams, ConversationListResult,
    ConversationMutationResult, ConversationObserveCursor, ConversationObserveParams,
    ConversationObserveResult, ConversationPermissionAnswerErrorDetails,
    ConversationPermissionSelectionState, ConversationReadParams, ConversationReceiptParams,
    ConversationReceiptResult, ConversationRemoveParams, ConversationReorderParams,
    ConversationSendParams, ConversationSetApprovalModeParams, ConversationSetApprovalModeResult,
    ConversationStopParams, ConversationSummary, ConversationView as WireConversationView,
};
use nessa_protocol::product::passive_read::decimal_u64;
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_protocol::{
    agents::AgentId,
    conversation::domain::{ConversationApprovalMode, ConversationId},
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    commands::{
        CreationFailure, CreationReceipt, CreationStage, CreationStorageError, MutationFailure,
        MutationOutcome, MutationReceipt, MutationStage,
    },
    permissions::PermissionSelectionState,
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
                let store = service.commands().ok_or(ConversationError::Unavailable)?;
                service
                    .create_command(
                        store,
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        requested_conversation(&params),
                    )
                    .await
                    .map_err(creation_failure)?;
                Ok(success(
                    &frame.id,
                    &ConversationCreateResult {
                        conversation_id: params.conversation_id,
                    },
                ))
            }
            "conversation.read" => {
                let params = params!(ConversationReadParams);
                let id = conversation_id(&params.conversation_id)?;
                let (view, _) = read_view(state, service, session, &id, &frame.id).await?;
                Ok(success(&frame.id, &view))
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
                let listed = read_list(service, session, archived.unwrap_or(false), &frame.id).await?;
                Ok(success(&frame.id, &listed))
            }
            "conversation.observe" => {
                let params = params!(ConversationObserveParams);
                let cursor = match params.cursor {
                    Some(cursor) => Some(observation_cursor(cursor)?),
                    None => None,
                };
                let observed = service
                    .observe(
                        caller(frame.id.clone()),
                        params.archived.unwrap_or(false),
                        cursor,
                    )
                    .await;
                trace_conversation_observe(observed.as_ref().err());
                Ok(success(&frame.id, &observe_result(observed?)))
            }
            "conversation.send" | "conversation.steer" => {
                let params = params!(ConversationSendParams);
                let mode = if frame.method == "conversation.steer" {
                    SubmissionMode::Steer
                } else {
                    SubmissionMode::Queue
                };
                let id = conversation_id(&params.conversation_id)?;
                let caller = caller(params.request_id.clone());
                let execution_id = params.execution_id.clone();
                let store = service.commands().ok_or(ConversationError::Unavailable)?;
                let submitted = service
                    .submit_command(
                        store,
                        id.clone(),
                        caller.clone(),
                        execution_id.clone(),
                        submitted_message(&params),
                        mode,
                    )
                    .await
                    .map_err(mutation_failure)?;
                let receipt = match submitted.delivery {
                    Some(receipt) => receipt,
                    None => service.current_submission(id, caller, execution_id).await?,
                };
                Ok(success(&frame.id, &receipt))
            }
            "conversation.stop" => {
                let params = params!(ConversationStopParams);
                let store = service.commands().ok_or(ConversationError::Unavailable)?;
                let receipt = service
                    .stop_command(
                        store,
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        params.execution_id,
                    )
                    .await
                    .map_err(mutation_failure)?;
                Ok(success(
                    &frame.id,
                    &command_receipt(&params.request_id, &receipt),
                ))
            }
            "conversation.receipt" => {
                let params = params!(ConversationReceiptParams);
                let store = service.commands().ok_or(ConversationError::Unavailable)?;
                let id = conversation_id(&params.conversation_id)?;
                let caller = caller(params.request_id.clone());
                let result = match params.operation {
                    ConversationCommandOperation::Create => service
                        .lookup_creation(
                            store,
                            id,
                            caller,
                            requested_conversation_from_receipt(&params)?,
                        )
                        .await
                        .map_err(creation_failure)?
                        .map(|receipt| command_receipt_from_creation(&params.request_id, &receipt)),
                    ConversationCommandOperation::Submit | ConversationCommandOperation::Steer => {
                        let message = submitted_message_from_receipt(&params)?;
                        let mode = if params.operation == ConversationCommandOperation::Steer {
                            SubmissionMode::Steer
                        } else {
                            SubmissionMode::Queue
                        };
                        let execution_id = params
                            .execution_id
                            .clone()
                            .ok_or(ConversationError::InvalidInput)?;
                        let binding =
                            crate::conversation::application::ConversationService::submit_lookup(
                                &id,
                                &caller,
                                &execution_id,
                                &message,
                                mode,
                            )?;
                        service
                            .lookup_command(store, id, caller, binding)
                            .await
                            .map_err(mutation_failure)?
                            .map(|receipt| command_receipt(&params.request_id, &receipt))
                    }
                    ConversationCommandOperation::Stop => {
                        let execution_id = params
                            .execution_id
                            .clone()
                            .ok_or(ConversationError::InvalidInput)?;
                        let binding =
                            crate::conversation::application::ConversationService::stop_lookup(
                                &id,
                                &caller,
                                &execution_id,
                            )?;
                        service
                            .lookup_command(store, id, caller, binding)
                            .await
                            .map_err(mutation_failure)?
                            .map(|receipt| command_receipt(&params.request_id, &receipt))
                    }
                };
                Ok(success(
                    &frame.id,
                    &ConversationReceiptResult {
                        found: result.is_some(),
                        request_id: params.request_id,
                        stage: result.as_ref().map(|receipt| receipt.stage),
                        outcome: result.and_then(|receipt| receipt.outcome),
                    },
                ))
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
/// The view `conversation.read` answers, with where in the committed history
/// it was folded through. A subscription's frame is this same read
/// (`docs/design/record-subscriptions.md`), so there is one read path.
pub(super) async fn read_view(
    state: &ProductRouteState,
    service: &ConversationService,
    session: &AuthenticatedSession,
    id: &ConversationId,
    request_id: &str,
) -> Result<(WireConversationView, Option<CommittedCursor>), ConversationError> {
    let read = service
        .read_at(id.clone(), caller(session, request_id.to_owned()))
        .await;
    // After the service answers, so a refusal keeps the error it returned.
    // The subscriber is the gateway log; nothing here changes the frame the
    // caller is sent.
    trace_conversation_read(id, read.as_ref().err());
    let (view, cursor) = read?;
    Ok((wire_view(state, view)?, cursor))
}

/// The list `conversation.list` answers; a list subscription's frame is this
/// same read.
pub(super) async fn read_list(
    service: &ConversationService,
    session: &AuthenticatedSession,
    archived: bool,
    request_id: &str,
) -> Result<ConversationListResult, ConversationError> {
    let listed = service
        .list(caller(session, request_id.to_owned()), archived)
        .await;
    // The desktop's index asks this list first. An incomplete list continues
    // as conversation.observe. The subject tells a list from a read.
    trace_conversation_index(listed.as_ref().err());
    Ok(list_result(listed?))
}

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

fn requested_conversation(params: &ConversationCreateParams) -> RequestedConversation {
    RequestedConversation {
        agent: requested_agent(params.agent.as_deref()),
        model: params.model.clone(),
        approval_mode: params.approval_mode.map(approval_mode),
    }
}
fn requested_conversation_from_receipt(
    params: &ConversationReceiptParams,
) -> Result<RequestedConversation, ConversationError> {
    if params.execution_id.is_some()
        || params.text.is_some()
        || params.attachments.is_some()
        || params.files.is_some()
    {
        return Err(ConversationError::InvalidInput);
    }
    Ok(RequestedConversation {
        agent: requested_agent(params.agent.as_deref()),
        model: params.model.clone(),
        approval_mode: params.approval_mode.map(approval_mode),
    })
}
fn approval_mode(mode: WireApprovalMode) -> ConversationApprovalMode {
    match mode {
        WireApprovalMode::Ask => ConversationApprovalMode::Ask,
        WireApprovalMode::Auto => ConversationApprovalMode::Auto,
        WireApprovalMode::Full => ConversationApprovalMode::Full,
    }
}
fn submitted_message(params: &ConversationSendParams) -> SubmittedMessage {
    SubmittedMessage {
        text: params.text.clone(),
        images: params
            .attachments
            .iter()
            .map(|image| SubmittedImage {
                digest: image.digest.clone(),
                media_type: image.mime_type.clone(),
                size: image.size,
            })
            .collect(),
        files: params
            .files
            .iter()
            .map(|file| SubmittedFile {
                path: file.path.clone(),
            })
            .collect(),
    }
}
fn submitted_message_from_receipt(
    params: &ConversationReceiptParams,
) -> Result<SubmittedMessage, ConversationError> {
    if params.agent.is_some() || params.model.is_some() || params.approval_mode.is_some() {
        return Err(ConversationError::InvalidInput);
    }
    Ok(SubmittedMessage {
        text: params.text.clone().ok_or(ConversationError::InvalidInput)?,
        images: params
            .attachments
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|image| SubmittedImage {
                digest: image.digest.clone(),
                media_type: image.mime_type.clone(),
                size: image.size,
            })
            .collect(),
        files: params
            .files
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|file| SubmittedFile {
                path: file.path.clone(),
            })
            .collect(),
    })
}
fn command_receipt(request_id: &str, receipt: &MutationReceipt) -> ConversationCommandReceipt {
    ConversationCommandReceipt {
        request_id: request_id.to_owned(),
        stage: match receipt.stage() {
            MutationStage::Accepted => ConversationCommandStage::Accepted,
            MutationStage::Attempted => ConversationCommandStage::Attempted,
            MutationStage::Settled => ConversationCommandStage::Settled,
        },
        outcome: receipt.outcome().map(|outcome| match outcome {
            MutationOutcome::Dispatched => ConversationCommandOutcome::Dispatched,
            MutationOutcome::Withdrawn => ConversationCommandOutcome::Withdrawn,
            MutationOutcome::Cancelled => ConversationCommandOutcome::Cancelled,
            MutationOutcome::AlreadyFinal => ConversationCommandOutcome::AlreadyFinal,
        }),
    }
}
fn command_receipt_from_creation(
    request_id: &str,
    receipt: &CreationReceipt,
) -> ConversationCommandReceipt {
    ConversationCommandReceipt {
        request_id: request_id.to_owned(),
        stage: match receipt.stage() {
            CreationStage::Accepted => ConversationCommandStage::Accepted,
            CreationStage::Attempted => ConversationCommandStage::Attempted,
            CreationStage::Ready => ConversationCommandStage::Ready,
        },
        outcome: None,
    }
}
fn creation_failure(failure: CreationFailure<ConversationError>) -> ConversationError {
    match failure {
        CreationFailure::Target(error) => error,
        CreationFailure::Conflict => ConversationError::Agent(AgentError::SubmissionConflict),
        CreationFailure::Interrupted(_) => {
            ConversationError::Agent(AgentError::SubmissionUnresolved)
        }
        CreationFailure::Storage(CreationStorageError::Storage(error)) => {
            ConversationError::Storage(error)
        }
        CreationFailure::Storage(_) | CreationFailure::TaskFault(_) => {
            ConversationError::Unavailable
        }
    }
}
fn mutation_failure(failure: MutationFailure<ConversationError>) -> ConversationError {
    match failure {
        MutationFailure::Target(error) => error,
        MutationFailure::Conflict => ConversationError::Agent(AgentError::SubmissionConflict),
        MutationFailure::Interrupted(_) => {
            ConversationError::Agent(AgentError::SubmissionUnresolved)
        }
        MutationFailure::Storage(CreationStorageError::Storage(error)) => {
            ConversationError::Storage(error)
        }
        MutationFailure::Storage(_) | MutationFailure::TaskFault(_) => {
            ConversationError::Unavailable
        }
    }
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

fn observation_cursor(
    cursor: ConversationObserveCursor,
) -> Result<ConversationObservationCursor, ConversationError> {
    Ok(ConversationObservationCursor {
        incarnation: cursor.incarnation,
        boundary: decimal_u64(&cursor.boundary).map_err(|_| ConversationError::InvalidInput)?,
        creation: decimal_u64(&cursor.creation).map_err(|_| ConversationError::InvalidInput)?,
        id: conversation_id(&cursor.id)?.to_string(),
    })
}

fn observe_result(observed: ConversationObservation) -> ConversationObserveResult {
    ConversationObserveResult {
        conversations: observed
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
        complete: observed.complete,
        cursor: observed.cursor.map(|cursor| ConversationObserveCursor {
            incarnation: cursor.incarnation,
            boundary: cursor.boundary.to_string(),
            creation: cursor.creation.to_string(),
            id: cursor.id,
        }),
    }
}

/// A `conversation.read` on the process tracing subscriber.
///
/// The ask is a debug event on a `conversation.read` span. A watched
/// conversation is read every poll, so the ask stays out of the default
/// gateway log; the span is info, and a refusal is a warning on it, so that
/// log keeps the conversation, the wire code, and a hint the code collapses
/// — busy, a socket — and never the error's text.
fn trace_conversation_read(id: &ConversationId, error: Option<&ConversationError>) {
    let span = tracing::info_span!(
        "conversation.read",
        conversation_id = %id,
        method = "conversation.read",
        subject = "conversation",
    );
    let _entered = span.enter();
    tracing::debug!("conversation read asked");
    let Some(error) = error else {
        return;
    };
    tracing::warn!(
        conversation_id = %id,
        method = "conversation.read",
        subject = "conversation",
        code = error_code(error).as_str(),
        hint = read_refusal_hint(error),
        "conversation read refused",
    );
}

/// A `conversation.list` — the desktop's index — on the same subscriber.
fn trace_conversation_index(error: Option<&ConversationError>) {
    let span = tracing::info_span!(
        "conversation.list",
        method = "conversation.list",
        subject = "index",
    );
    let _entered = span.enter();
    tracing::debug!("conversation index asked");
    let Some(error) = error else {
        return;
    };
    tracing::warn!(
        method = "conversation.list",
        subject = "index",
        code = error_code(error).as_str(),
        hint = read_refusal_hint(error),
        "conversation index refused",
    );
}

/// A `conversation.observe` page — the rest of the desktop's index — on the
/// same subscriber. The list span stays the bounded newest-first read.
fn trace_conversation_observe(error: Option<&ConversationError>) {
    let span = tracing::info_span!(
        "conversation.observe",
        method = "conversation.observe",
        subject = "index",
    );
    let _entered = span.enter();
    tracing::debug!("conversation index page asked");
    let Some(error) = error else {
        return;
    };
    tracing::warn!(
        method = "conversation.observe",
        subject = "index",
        code = error_code(error).as_str(),
        hint = read_refusal_hint(error),
        "conversation index page refused",
    );
}

/// A static hint already on the error, where the wire code collapses several
/// causes into one. Absent when the code itself is the whole story. Never a
/// `Display` string: those carry transport text.
fn read_refusal_hint(error: &ConversationError) -> Option<&'static str> {
    match error {
        ConversationError::Unavailable => Some("unavailable"),
        ConversationError::TurnRunning => Some("turn-running"),
        ConversationError::Capacity => Some("capacity"),
        ConversationError::Agent(error) => agent_read_hint(error),
        _ => None,
    }
}

fn agent_read_hint(error: &AgentError) -> Option<&'static str> {
    match error {
        AgentError::Busy => Some("busy"),
        AgentError::Transport(_) => Some("socket"),
        AgentError::Closed => Some("closed"),
        AgentError::Deadline | AgentError::StartupDeadline(_) => Some("deadline"),
        AgentError::Backpressure => Some("backpressure"),
        AgentError::AttachmentUnavailable(_) => Some("attachment"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/read_trace.rs"]
mod read_trace;
#[cfg(test)]
#[path = "../../tests/conversation/wire_errors.rs"]
mod tests;
#[cfg(test)]
#[path = "../../tests/conversation/wire_list.rs"]
mod wire_list;
