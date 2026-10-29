//! Authenticated wire commands translate into the server-owned conversation service.
//! The socket has already checked current access and the conversation action grant.
use super::{
    read_access::CatalogueReader,
    socket::{failure, failure_with_details, success},
    state::ProductRouteState,
};
use crate::conversation::application::{
    error_code, rule_principal, CatalogueHead, ConversationCaller, ConversationError,
    ConversationService, ListedPermissionRule, PermissionRuleCatalog, PreferenceUpdate,
    QuestionChoiceInput, ReadOpening, RequestedAgent, RequestedConversation, RuleCatalogError,
    RuleChange, RulePrincipal, SubmissionMode, SubmittedFile, SubmittedImage, SubmittedMessage,
    SubmittedUpload, UsageLimitPreferenceFacts, UsageLimitPreferenceGrant,
};
use base64::Engine as _;
use nessa_auth::application::ports::{AccessError, AccessSnapshot};
use nessa_auth::application::session::{AuthenticatedSession, ReadCurrentSession};
use nessa_auth::domain::{MembershipRole, Resource};
use nessa_protocol::conversation::projection::CommittedCursor;
use nessa_protocol::conversation::view::{
    ConversationList, ConversationModelChangeEffect, ConversationObservation,
    ConversationObservationCursor, ConversationPartPosition, ConversationPoint,
    ConversationView as ApplicationConversationView,
};
use nessa_protocol::product::generated::{
    ApprovalMode as WireApprovalMode, ConversationAnswerParams, ConversationAnswerQuestionParams,
    ConversationArchiveParams, ConversationCancelParams, ConversationCloseParams,
    ConversationCommandOperation, ConversationCommandOutcome, ConversationCommandReceipt,
    ConversationCommandStage, ConversationCreateParams, ConversationCreateResult,
    ConversationDeleteParams, ConversationForkParams, ConversationListParams,
    ConversationListResult, ConversationMutationResult, ConversationObserveCursor,
    ConversationObserveParams, ConversationObserveResult, ConversationPermissionAnswerErrorDetails,
    ConversationPermissionReviewParams, ConversationPermissionReviewResult,
    ConversationPermissionRulesParams, ConversationPermissionRulesResult,
    ConversationPermissionSelectionState, ConversationQuestionReviewParams,
    ConversationQuestionReviewResult, ConversationReadEarlierParams, ConversationReadParams,
    ConversationReceiptParams, ConversationReceiptResult, ConversationRemoveParams,
    ConversationRenameParams, ConversationReorderParams, ConversationRevokePermissionRuleParams,
    ConversationRevokePermissionRuleResult, ConversationSendParams,
    ConversationSetApprovalModeParams, ConversationSetApprovalModeResult,
    ConversationSetModelParams, ConversationSetModelResult,
    ConversationSetUsageLimitAutoContinueParams, ConversationSetUsageLimitAutoContinueResult,
    ConversationStopParams, ConversationSummary, ConversationSwitchAgentParams,
    ConversationView as WireConversationView, ModelChangeEffect, PermissionRule,
    PermissionRuleEffect, MAX_REVIEW_RANGE_BYTES,
};
use nessa_protocol::product::generated::{
    ConversationMessageDelivery, ConversationMessageParams, ConversationMessageReceipt,
    ConversationMessageRetiredReason, ConversationMessageState, ConversationMessageStatusParams,
    ConversationMessageStatusResult,
};
use nessa_protocol::product::passive_read::decimal_u64;
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame, ResponseFrame, MAX_PAYLOAD_BYTES};
use nessa_protocol::{
    agents::AgentId,
    conversation::domain::{
        ConversationApprovalMode, ConversationId, ConversationModelId, ConversationTitle,
    },
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    commands::{
        CreationFailure, CreationReceipt, CreationStage, CreationStorageError, MutationFailure,
        MutationOutcome, MutationReceipt, MutationStage,
    },
    permissions::{is_remember_option, PermissionCaller, PermissionSelectionState},
    sessions::{NoteDelivery, NoteRetirementReason, NoteState},
};
use nessa_sdk::domain::agent_execution::executions::Initiation;
use serde_json::{json, Value};

/// `initiation` is who a turn this request starts is saved as started by,
/// as the request was admitted (`read_access::admit_request`).
pub(crate) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    initiation: Initiation,
) -> OutgoingMessage {
    let method = frame.method.clone();
    let request_id = frame.id.clone();
    match answer(state, session, frame, initiation).await {
        Ok(response) => response,
        Err(error) => {
            trace_refusal_causes(&method, &error);
            match error {
                ConversationError::PermissionAnswer { error, selection } => {
                    permission_answer_failure(&request_id, error, selection)
                }
                error => failure(&request_id, error_code(&error).as_str()),
            }
        }
    }
}

/// The command's answer, or the refusal `dispatch` answers and logs. A
/// check that answers its own code returns it as a response.
async fn answer(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
    initiation: Initiation,
) -> Result<OutgoingMessage, ConversationError> {
    if matches!(
        frame.method.as_str(),
        "conversation.permissionRules" | "conversation.revokePermissionRule"
    ) {
        return Ok(dispatch_permission_rules(state, session, frame).await);
    }
    // Catalogue refusal precedes service availability (row L12). Retain the
    // admitted scope for the read rather than resolving the binding again.
    let list_scope = if matches!(
        frame.method.as_str(),
        "conversation.list" | "conversation.observe"
    ) {
        match super::read_access::admit_list(state, session).await {
            Ok(scope) => Some(scope),
            Err(code) => return Ok(failure(&frame.id, code)),
        }
    } else {
        None
    };
    // This build runs no conversations at all, which is not the same fact as a
    // caller naming an agent this one is not configured for. Sharing a code
    // between them made the panel tell someone with a working Claude that the
    // gateway has no agent configured.
    let Some(service) = state.conversations.as_ref() else {
        // Not `agent_not_configured`: this gateway runs no conversations at
        // all, and telling a user with a working agent to go configure one
        // sends them to change something that was never the problem.
        return Ok(failure(
            &frame.id,
            ConversationErrorCode::ConversationsNotConfigured.as_str(),
        ));
    };
    macro_rules! params {
        ($kind:ty) => {
            match serde_json::from_value::<$kind>(frame.params) {
                Ok(value) => value,
                Err(_) => return Err(ConversationError::InvalidInput),
            }
        };
    }
    macro_rules! target_params {
        ($kind:ty) => {
            target_params!($kind, crate::conversation::application::AccessAction::Act)
        };
        ($kind:ty, $when_deleted:expr) => {{
            let value = params!($kind);
            let id = conversation_id(&value.conversation_id)?;
            if let Err(code) =
                super::read_access::admit_command_target(state, session, &id, $when_deleted).await
            {
                return Ok(failure(&frame.id, code));
            }
            value
        }};
    }
    let caller = |request_id: String| caller(session, request_id);
    let result: Result<OutgoingMessage, ConversationError> = async {
        match frame.method.as_str() {
            "conversation.create" => {
                let params = target_params!(ConversationCreateParams);
                // The phone check judges the preset the conversation starts
                // in, which a channel's defaults can set.
                let default_mode = service
                    .creation_default_mode(
                        &caller(params.request_id.clone()),
                        requested_channel(params.channel_id.as_deref())?.as_ref(),
                    )
                    .await?;
                if let Err(code) = super::owner_phone::admit_creation(
                    state,
                    session,
                    params
                        .approval_mode
                        .map(approval_mode)
                        .map(|mode| mode.as_str()),
                    default_mode.as_str(),
                )
                .await
                {
                    return Ok(failure(&frame.id, code));
                }
                let store = service.commands().ok_or(ConversationError::Unavailable)?;
                service
                    .create_command(
                        store,
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        requested_conversation(&params)?,
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
            "conversation.peek" => {
                let params = target_params!(ConversationReadParams);
                let id = conversation_id(&params.conversation_id)?;
                if let Err(code) = super::read_access::admit_conversation(state, session, &id).await
                {
                    return Ok(failure(&frame.id, code));
                }
                let execution = service
                    .active_execution(id, caller(frame.id.clone()))
                    .await?;
                Ok(success(
                    &frame.id,
                    &json!({
                        "conversationId": params.conversation_id,
                        "executionId": execution,
                    }),
                ))
            }
            "conversation.readEarlier" => {
                let params = target_params!(ConversationReadEarlierParams);
                let id = conversation_id(&params.conversation_id)?;
                if let Err(code) = super::read_access::admit_conversation(state, session, &id).await
                {
                    return Ok(failure(&frame.id, code));
                }
                let before = point(params.before)?;
                let after = params.after.map(point).transpose()?;
                let page = service
                    .read_earlier(&id, &caller(frame.id.clone()), &before, after.as_ref())
                    .await?;
                Ok(success(&frame.id, &page))
            }
            "conversation.read" => {
                let params = target_params!(ConversationReadParams);
                let id = conversation_id(&params.conversation_id)?;
                if let Err(code) = super::read_access::admit_conversation(state, session, &id).await
                {
                    return Ok(failure(&frame.id, code));
                }
                let (view, _) =
                    read_view(state, service, session, &id, &frame.id, ReadOpening::Open).await?;
                Ok(success(&frame.id, &view))
            }
            "conversation.setModel" => {
                let params = target_params!(ConversationSetModelParams);
                let selected = service
                    .set_model(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        ConversationModelId::new(params.model)
                            .map_err(|_| ConversationError::InvalidInput)?,
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationSetModelResult {
                        request_id: params.request_id,
                        model: selected.model.as_str().into(),
                        approval_mode: match selected.approval_mode {
                            ConversationApprovalMode::Ask => WireApprovalMode::Ask,
                            ConversationApprovalMode::Auto => WireApprovalMode::Auto,
                            ConversationApprovalMode::Full => WireApprovalMode::Full,
                        },
                        effect: match selected.effect {
                            ConversationModelChangeEffect::SubsequentResponses => {
                                ModelChangeEffect::SubsequentResponses
                            }
                            ConversationModelChangeEffect::NextTurn => ModelChangeEffect::NextTurn,
                        },
                    },
                ))
            }
            "conversation.setUsageLimitAutoContinue" => {
                let params = params!(ConversationSetUsageLimitAutoContinueParams);
                let snapshot = match (ReadCurrentSession {
                    access: state.access.as_ref(),
                    clock: state.clock.as_ref(),
                })
                .execute(session)
                .await
                {
                    Ok(snapshot) => snapshot,
                    Err(AccessError::Unavailable | AccessError::Unsupported) => {
                        return Err(ConversationError::Unavailable);
                    }
                    Err(_) => return Ok(failure(&frame.id, "forbidden")),
                };
                let facts = usage_limit_preference_facts(&snapshot, &state.gateway);
                match service
                    .set_usage_limit_auto_continue(
                        params.enabled,
                        caller(params.request_id.clone()),
                        facts,
                    )
                    .await?
                {
                    PreferenceUpdate::Stored(enabled) => Ok(success(
                        &frame.id,
                        &ConversationSetUsageLimitAutoContinueResult {
                            request_id: params.request_id,
                            enabled,
                        },
                    )),
                    PreferenceUpdate::Refused { .. } => Ok(failure(&frame.id, "forbidden")),
                    PreferenceUpdate::Pending => Ok(failure(&frame.id, "unavailable")),
                }
            }
            "conversation.switchAgent" => {
                let params = target_params!(ConversationSwitchAgentParams);
                let agent = AgentId::parse(&params.agent).ok_or(ConversationError::InvalidInput)?;
                let applied = service
                    .switch_agent(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        agent,
                        ConversationModelId::new(params.model)
                            .map_err(|_| ConversationError::InvalidInput)?,
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
            "conversation.fork" => {
                let params = target_params!(ConversationForkParams);
                // The copy reads the source, so its read admission comes
                // first; the service then admits only the source's owner.
                let source = conversation_id(&params.source_conversation_id)?;
                if let Err(code) =
                    super::read_access::admit_conversation(state, session, &source).await
                {
                    return Ok(failure(&frame.id, code));
                }
                let store = service.commands().ok_or(ConversationError::Unavailable)?;
                service
                    .fork(
                        store,
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        source,
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
            "conversation.setApprovalMode" => {
                let params = target_params!(ConversationSetApprovalModeParams);
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
                let scope = list_scope.as_ref().ok_or(ConversationError::Metadata)?;
                let (listed, _) = read_list(
                    state,
                    service,
                    session,
                    archived.unwrap_or(false),
                    &frame.id,
                    scope,
                )
                .await?;
                Ok(success(&frame.id, &fitted_list(listed, &frame.id)))
            }
            "conversation.observe" => {
                let params = params!(ConversationObserveParams);
                let scope = list_scope.as_ref().ok_or(ConversationError::Metadata)?;
                let cursor = match params.cursor {
                    Some(cursor) => Some(observation_cursor(cursor)?),
                    None => None,
                };
                let observed = service
                    .observe_for(
                        caller(frame.id.clone()),
                        params.archived.unwrap_or(false),
                        cursor,
                        &scope.reader,
                        &scope.owner,
                        scope.policy.as_deref(),
                    )
                    .await;
                trace_conversation_observe(observed.as_ref().err());
                let observed = observe_result(observed?);
                super::read_access::admit_index_rows(state, session, &observed.conversations)
                    .await?;
                Ok(success(&frame.id, &observed))
            }
            "conversation.send" | "conversation.steer" => {
                let params = target_params!(ConversationSendParams);
                if let Err(code) =
                    super::owner_phone::admit_submission_input(state, session, params.files.len())
                        .await
                {
                    return Ok(failure(&frame.id, code));
                }
                let mode = if frame.method == "conversation.steer" {
                    SubmissionMode::Steer
                } else {
                    SubmissionMode::Queue
                };
                let id = conversation_id(&params.conversation_id)?;
                let caller = caller(params.request_id.clone());
                let execution_id = params.execution_id.clone();
                let store = service.commands().ok_or(ConversationError::Unavailable)?;
                // Admission still starts an unpaired session as an owner turn.
                // Whether that turn may offer a remember choice is this
                // membership, and a membership that cannot be read offers none.
                // An owner phone asks `conversation.remember` for the same
                // choice; that purpose forbids it, so the turn offers none.
                let offers_remember = authenticated_rule_principal(state, session)
                    .await
                    .is_ok_and(|principal| principal == RulePrincipal::Owner)
                    && super::owner_phone::admit_remember(state, session)
                        .await
                        .is_ok();
                let submitted = service
                    .submit_command(
                        store,
                        id.clone(),
                        caller.clone(),
                        initiation,
                        offers_remember,
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
                let params = target_params!(ConversationStopParams);
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
                let params = target_params!(ConversationReceiptParams);
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
                        if has_creation_hints(&params)
                            || params.text.is_some()
                            || params.attachments.is_some()
                            || params.files.is_some()
                            || params.uploads.is_some()
                        {
                            return Err(ConversationError::InvalidInput);
                        }
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
            "conversation.message" => {
                let params = target_params!(ConversationMessageParams);
                let (note, state) = service
                    .leave_note(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        initiation.initiator().clone(),
                        params.text,
                        match params.delivery {
                            ConversationMessageDelivery::RecordOnly => NoteDelivery::RecordOnly,
                            ConversationMessageDelivery::NextTurn => NoteDelivery::NextTurn,
                        },
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationMessageReceipt {
                        request_id: params.request_id,
                        message_id: note.id.as_str().to_owned(),
                        accepted_at_ms: note.accepted_at_ms,
                        delivery_state: note_state(&state),
                        execution_id: assigned_turn(&state),
                        retired_reason: retired_reason(&state),
                    },
                ))
            }
            "conversation.messageStatus" => {
                let params = target_params!(ConversationMessageStatusParams);
                let found = service
                    .note_status(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationMessageStatusResult {
                        found: found.is_some(),
                        request_id: params.request_id,
                        message_id: found.as_ref().map(|(note, _)| note.id.as_str().to_owned()),
                        accepted_at_ms: found.as_ref().map(|(note, _)| note.accepted_at_ms),
                        delivery_state: found.as_ref().map(|(_, state)| note_state(state)),
                        execution_id: found.as_ref().and_then(|(_, state)| assigned_turn(state)),
                        retired_reason: found.as_ref().and_then(|(_, state)| retired_reason(state)),
                    },
                ))
            }
            "conversation.reorder" => {
                let params = target_params!(ConversationReorderParams);
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
                let params = target_params!(ConversationRemoveParams);
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
                let params = target_params!(ConversationAnswerQuestionParams);
                interaction_key(&params.question)?;
                service
                    .answer_question(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        params.execution_id,
                        params.question_id,
                        params.question,
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
                let params = target_params!(ConversationAnswerParams);
                interaction_key(&params.review)?;
                // A remember choice is its own purpose decision, asked before
                // the permission-rule catalog. An ordinary once answer does
                // not ask it, so a failed membership read still answers once.
                if is_remember_option(&params.option_id) {
                    if let Err(code) = super::owner_phone::admit_remember(state, session).await {
                        return Ok(failure(&frame.id, code));
                    }
                }
                // Only a remember choice reads the membership. An ordinary
                // once answer does not fail when that read is unavailable.
                let permission_caller = match permission_caller_for_option(
                    &params.option_id,
                    if is_remember_option(&params.option_id) {
                        authenticated_rule_principal(state, session).await
                    } else {
                        Ok(RulePrincipal::Grant)
                    },
                ) {
                    Ok(caller) => caller,
                    Err(code) => return Ok(failure(&frame.id, code)),
                };
                // Cedar admits or refuses the caller who is answering. A grant
                // is not rewritten as the owner, and a refusal writes nothing.
                if is_remember_option(&params.option_id) {
                    if let Some(rules) = state.permission_rules.as_deref() {
                        if let Err(error) =
                            rules.authorize(rule_principal(permission_caller), RuleChange::Grant)
                        {
                            return Ok(failure(&frame.id, catalog_code(error)));
                        }
                    }
                }
                service
                    .answer(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        params.execution_id,
                        params.permission_id,
                        params.review,
                        params.option_id,
                        permission_caller,
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
            "conversation.permissionReview" => {
                let params = target_params!(ConversationPermissionReviewParams);
                let max_bytes = usize::try_from(params.max_bytes)
                    .ok()
                    .filter(|bytes| (1..=MAX_REVIEW_RANGE_BYTES).contains(bytes))
                    .ok_or(ConversationError::InvalidInput)?;
                interaction_key(&params.review)?;
                let (data, total_bytes) = service
                    .permission_review(
                        conversation_id(&params.conversation_id)?,
                        caller(frame.id.clone()),
                        params.review,
                        params.offset,
                        max_bytes,
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationPermissionReviewResult {
                        offset: params.offset,
                        total_bytes,
                        data: base64::engine::general_purpose::STANDARD.encode(data),
                    },
                ))
            }
            "conversation.questionReview" => {
                let params = target_params!(ConversationQuestionReviewParams);
                let max_bytes = usize::try_from(params.max_bytes)
                    .ok()
                    .filter(|bytes| (1..=MAX_REVIEW_RANGE_BYTES).contains(bytes))
                    .ok_or(ConversationError::InvalidInput)?;
                interaction_key(&params.question)?;
                let (data, total_bytes) = service
                    .question_review(
                        conversation_id(&params.conversation_id)?,
                        caller(frame.id.clone()),
                        params.question,
                        params.offset,
                        max_bytes,
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationQuestionReviewResult {
                        offset: params.offset,
                        total_bytes,
                        data: base64::engine::general_purpose::STANDARD.encode(data),
                    },
                ))
            }
            "conversation.cancel" => {
                let params = target_params!(ConversationCancelParams);
                interaction_key(&params.review)?;
                service
                    .cancel_permission(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        params.execution_id,
                        params.permission_id,
                        params.review,
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
                let params = target_params!(ConversationCloseParams);
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
                let params = target_params!(ConversationArchiveParams);
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
            "conversation.rename" => {
                let params = target_params!(ConversationRenameParams);
                // The domain owns what a title may be; the schema only bounds it.
                let title = ConversationTitle::new(&params.title)
                    .map_err(|_| ConversationError::InvalidInput)?;
                let applied = service
                    .rename(
                        conversation_id(&params.conversation_id)?,
                        caller(params.request_id.clone()),
                        title,
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
                let params = target_params!(
                    ConversationDeleteParams,
                    crate::conversation::application::AccessAction::Erase
                );
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
    result
}

/// A refused conversation command — any method, reads included — as one
/// warning in the gateway log, at the socket's answer to every request,
/// passive record reads included: admission, share commands, a check answering its own code, and an error
/// alike. Only the method and the wire code; the causes a code keeps only
/// one of are the line `trace_refusal_causes` writes beside it. A read's own
/// span may also have warned.
pub(super) fn trace_refused_command(method: &str, response: &OutgoingMessage) {
    if !method.starts_with("conversation.") {
        return;
    }
    let OutgoingMessage::Response(ResponseFrame {
        ok: false,
        error: Some(error),
        ..
    }) = response
    else {
        return;
    };
    tracing::warn!(
        method,
        code = error.code.as_str(),
        "conversation command refused",
    );
}

/// Every cause behind a refusal answered as an error, beside its
/// `conversation command refused` line: the wire code keeps only one.
fn trace_refusal_causes(method: &str, error: &ConversationError) {
    tracing::warn!(
        method,
        code = error_code(error).as_str(),
        causes = %super::refusal_causes::refusal_causes(error),
        "conversation command refusal causes",
    );
}

/// Refuse a review's or a question's key that is not 32 lowercase hex
/// digits, as the view lists it.
fn interaction_key(key: &str) -> Result<(), ConversationError> {
    if key.len() == 32
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(ConversationError::InvalidInput)
    }
}

/// List and revoke remembered rules. Who may is the authenticated
/// membership, not the turn-initiation category admission used.
async fn dispatch_permission_rules(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let principal = match authenticated_rule_principal(state, session).await {
        Ok(principal) => principal,
        Err(code) => return failure(&frame.id, code),
    };
    let rules = match remembered_rules(state, &frame.id) {
        Ok(rules) => rules,
        Err(response) => return *response,
    };
    match frame.method.as_str() {
        "conversation.permissionRules" => {
            let Ok(params) =
                serde_json::from_value::<ConversationPermissionRulesParams>(frame.params)
            else {
                return failure(&frame.id, ConversationErrorCode::InvalidRequest.as_str());
            };
            match rules.list(principal) {
                Ok(listed) => success(
                    &frame.id,
                    &ConversationPermissionRulesResult {
                        request_id: params.request_id,
                        rules: listed.into_iter().filter_map(listed_rule).collect(),
                    },
                ),
                Err(error) => failure(&frame.id, catalog_code(error)),
            }
        }
        "conversation.revokePermissionRule" => {
            let Ok(params) =
                serde_json::from_value::<ConversationRevokePermissionRuleParams>(frame.params)
            else {
                return failure(&frame.id, ConversationErrorCode::InvalidRequest.as_str());
            };
            let Ok(actor) = caller(session, params.request_id.clone()).actor() else {
                return failure(&frame.id, ConversationErrorCode::InvalidRequest.as_str());
            };
            match rules.revoke(&params.rule_id, principal, &actor) {
                Ok(applied) => success(
                    &frame.id,
                    &ConversationRevokePermissionRuleResult {
                        request_id: params.request_id,
                        applied,
                    },
                ),
                Err(error) => failure(&frame.id, catalog_code(error)),
            }
        }
        _ => failure(&frame.id, ConversationErrorCode::UnknownMethod.as_str()),
    }
}

/// The membership Cedar's rule policy is asked about.
///
/// An admin is the local owner or one of that owner's surfaces. A member is
/// a grant, which the rule policy refuses. Admission maps every unpaired
/// session to an owner turn; that category is not this decision.
async fn authenticated_rule_principal(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<RulePrincipal, &'static str> {
    let snapshot = ReadCurrentSession {
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
    }
    .execute(session)
    .await
    .map_err(|_| "temporarily_unavailable")?;
    Ok(match snapshot.membership.role() {
        MembershipRole::Admin => RulePrincipal::Owner,
        MembershipRole::Member => RulePrincipal::Grant,
    })
}

/// Who an answer is recorded as.
///
/// A remember choice needs the authenticated membership. Any other option
/// does not: a failed membership read must not refuse an ordinary once answer.
fn permission_caller_for_option(
    option_id: &str,
    principal: Result<RulePrincipal, &'static str>,
) -> Result<PermissionCaller, &'static str> {
    if !is_remember_option(option_id) {
        return Ok(PermissionCaller::Grant);
    }
    principal.map(noted_permission_caller)
}

fn noted_permission_caller(principal: RulePrincipal) -> PermissionCaller {
    match principal {
        RulePrincipal::Owner => PermissionCaller::Owner,
        RulePrincipal::Grant => PermissionCaller::Grant,
    }
}

/// The remembered rules, or the refusal when this gateway has none.
///
/// Who may read or change them is Cedar's answer, from the authenticated
/// membership. This does not decide that.
fn remembered_rules<'a>(
    state: &'a ProductRouteState,
    request_id: &str,
) -> Result<&'a crate::conversation::infrastructure::PermissionMemory, Box<OutgoingMessage>> {
    state.permission_rules.as_deref().ok_or_else(|| {
        Box::new(failure(
            request_id,
            ConversationErrorCode::ConversationsNotConfigured.as_str(),
        ))
    })
}

fn catalog_code(error: RuleCatalogError) -> &'static str {
    match error {
        RuleCatalogError::Refused => "forbidden",
        RuleCatalogError::Unavailable => "temporarily_unavailable",
    }
}

fn listed_rule(rule: ListedPermissionRule) -> Option<PermissionRule> {
    let effect = match rule.effect {
        "allow" => PermissionRuleEffect::Allow,
        "deny" => PermissionRuleEffect::Deny,
        _ => return None,
    };
    Some(PermissionRule {
        id: rule.id,
        revision: rule.revision,
        workspace: rule.workspace,
        source: rule.source,
        tool: rule.tool,
        effect,
    })
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
    opening: ReadOpening,
) -> Result<(WireConversationView, Option<CommittedCursor>), ConversationError> {
    let read = service
        .read_at(id.clone(), caller(session, request_id.to_owned()), opening)
        .await;
    // After the service answers, so a refusal keeps the error it returned.
    // The subscriber is the gateway log; nothing here changes the frame the
    // caller is sent.
    trace_conversation_read(id, read.as_ref().err());
    let (view, cursor) = read?;
    Ok((wire_view(state, view).await?, cursor))
}

/// The list `conversation.list` answers; a list subscription's frame is this
/// same read. Its callers have asked `read_access::admit_list`.
pub(super) async fn read_list(
    state: &ProductRouteState,
    service: &ConversationService,
    session: &AuthenticatedSession,
    archived: bool,
    request_id: &str,
    scope: &CatalogueReader,
) -> Result<(ConversationListResult, Option<CatalogueHead>), ConversationError> {
    let listed = service
        .list_for(
            caller(session, request_id.to_owned()),
            archived,
            &scope.reader,
            &scope.owner,
            scope.policy.as_deref(),
        )
        .await;
    // The desktop's index asks this list first. An incomplete list continues
    // as conversation.observe. The subject tells a list from a read.
    trace_conversation_index(listed.as_ref().err());
    let (listed, head) = listed?;
    let listed = list_result(listed);
    super::read_access::admit_index_rows(state, session, &listed.conversations).await?;
    Ok((listed, head))
}

fn usage_limit_preference_facts(
    snapshot: &AccessSnapshot,
    gateway: &Resource,
) -> UsageLimitPreferenceFacts {
    UsageLimitPreferenceFacts {
        principal_id: snapshot.credential.principal_id().as_str().to_owned(),
        organization_id: snapshot.membership.organization_id().as_str().to_owned(),
        role: match snapshot.membership.role() {
            MembershipRole::Admin => "admin",
            MembershipRole::Member => "member",
        }
        .to_owned(),
        active: snapshot.membership.is_active(),
        grants: snapshot
            .credential
            .grants()
            .iter()
            .map(|grant| UsageLimitPreferenceGrant {
                action: grant.action().as_str().to_owned(),
                resource_id: grant.resource().id().as_str().to_owned(),
                organization_id: grant.resource().organization_id().as_str().to_owned(),
            })
            .collect(),
        gateway_id: gateway.id().as_str().to_owned(),
        gateway_organization: gateway.organization_id().as_str().to_owned(),
    }
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

fn requested_conversation(
    params: &ConversationCreateParams,
) -> Result<RequestedConversation, ConversationError> {
    Ok(RequestedConversation {
        agent: requested_agent(params.agent.as_deref()),
        model: params.model.clone(),
        approval_mode: params.approval_mode.map(approval_mode),
        environment: params.environment.clone(),
        channel: requested_channel(params.channel_id.as_deref())?,
    })
}
/// A channel id as asked: one that is not an id is an invalid request.
fn requested_channel(
    channel: Option<&str>,
) -> Result<Option<crate::conversation::domain::ChannelId>, ConversationError> {
    channel
        .map(crate::conversation::domain::ChannelId::new)
        .transpose()
        .map_err(|_| ConversationError::InvalidInput)
}
fn requested_conversation_from_receipt(
    params: &ConversationReceiptParams,
) -> Result<RequestedConversation, ConversationError> {
    if params.execution_id.is_some()
        || params.text.is_some()
        || params.attachments.is_some()
        || params.files.is_some()
        || params.uploads.is_some()
    {
        return Err(ConversationError::InvalidInput);
    }
    Ok(RequestedConversation {
        agent: requested_agent(params.agent.as_deref()),
        model: params.model.clone(),
        approval_mode: params.approval_mode.map(approval_mode),
        environment: params.environment.clone(),
        channel: requested_channel(params.channel_id.as_deref())?,
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
        uploads: params
            .uploads
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|file| SubmittedUpload {
                digest: file.digest.clone(),
                media_type: file.mime_type.clone(),
                size: file.size,
                name: file.name.clone(),
            })
            .collect(),
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
/// A creation's hints, its environment among them, name a creation; a receipt
/// for any other operation that carries one is malformed, not a lookup.
fn has_creation_hints(params: &ConversationReceiptParams) -> bool {
    params.agent.is_some()
        || params.model.is_some()
        || params.approval_mode.is_some()
        || params.environment.is_some()
}
fn submitted_message_from_receipt(
    params: &ConversationReceiptParams,
) -> Result<SubmittedMessage, ConversationError> {
    if has_creation_hints(params) {
        return Err(ConversationError::InvalidInput);
    }
    Ok(SubmittedMessage {
        uploads: params
            .uploads
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|file| SubmittedUpload {
                digest: file.digest.clone(),
                media_type: file.mime_type.clone(),
                size: file.size,
                name: file.name.clone(),
            })
            .collect(),
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
/// Where a note stands, as the wire names it.
pub(super) fn note_state(state: &NoteState) -> ConversationMessageState {
    match state {
        NoteState::Recorded => ConversationMessageState::Recorded,
        NoteState::Pending => ConversationMessageState::Pending,
        NoteState::Assigned(_) => ConversationMessageState::Assigned,
        NoteState::Retired(_) => ConversationMessageState::Retired,
    }
}
/// The turn that took an assigned note.
pub(super) fn assigned_turn(state: &NoteState) -> Option<String> {
    match state {
        NoteState::Assigned(turn) => Some(turn.as_str().to_owned()),
        _ => None,
    }
}
/// Why a retired note was retired, as the wire names it.
pub(super) fn retired_reason(state: &NoteState) -> Option<ConversationMessageRetiredReason> {
    match state {
        NoteState::Retired(retirement) => Some(match retirement.reason {
            NoteRetirementReason::Expired => ConversationMessageRetiredReason::Expired,
            NoteRetirementReason::PermissionLost => {
                ConversationMessageRetiredReason::PermissionLost
            }
        }),
        _ => None,
    }
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
            MutationOutcome::NoEffect
            | MutationOutcome::RefusedBeforeAttempt
            | MutationOutcome::ModelChangedBeforeAttempt
            | MutationOutcome::AgentChangedBeforeAttempt => ConversationCommandOutcome::NoEffect,
            MutationOutcome::RefusedAfterSave => ConversationCommandOutcome::RefusedAfterSave,
            MutationOutcome::UnavailableAfterSave => {
                ConversationCommandOutcome::UnavailableAfterSave
            }
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

/// A point in a conversation's history, as a reader names it.
fn point(
    value: nessa_protocol::product::generated::ConversationPoint,
) -> Result<ConversationPoint, ConversationError> {
    let place = |value: u64| usize::try_from(value).map_err(|_| ConversationError::InvalidInput);
    Ok(ConversationPoint {
        execution_id: value.execution_id,
        at: value
            .at
            .map(|at| {
                Ok::<_, ConversationError>(ConversationPartPosition {
                    part: place(at.part)?,
                    byte: place(at.byte)?,
                })
            })
            .transpose()?,
    })
}

/// The authenticated catalog owns display facts and preset descriptions. The
/// application view owns the committed selection; the wire joins them once.
///
/// It is stamped with the gateway's own time as `serverNowMs`, read from the
/// view clock as the view is made: the timeline each deadline's `dueAtMs` was
/// promised on, so a client measures the deadline against that rather than
/// its own clock (`a_view_carries_the_gateways_time_from_its_clock`).
async fn wire_view(
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
    let conversation_id = view.conversation_id.clone();
    let mut value = serde_json::to_value(view).map_err(|_| ConversationError::Unavailable)?;
    let fields = value
        .as_object_mut()
        .ok_or(ConversationError::Unavailable)?;
    fields.insert(
        "serverNowMs".into(),
        json!(state.view_clock.unix_milliseconds()),
    );
    fields.insert(
        "approvalMode".into(),
        json!(selection.approval_mode.as_str()),
    );
    fields.insert("model".into(), json!(selection.model));
    fields.insert(
        "approvalModes".into(),
        serde_json::to_value(&model.approval_modes).map_err(|_| ConversationError::Unavailable)?,
    );
    if let Some(runtime) = fields.get_mut("runtime") {
        let runtime = runtime
            .as_object_mut()
            .ok_or(ConversationError::Unavailable)?;
        runtime.insert("agent".into(), json!(selection.agent.name()));
        runtime.insert("model".into(), json!(selection.model));
        runtime.insert("modelName".into(), json!(model.display_name));
        runtime.insert(
            "contextWindowTokens".into(),
            json!(model.max_context_window_tokens),
        );
        runtime.insert("reasoning".into(), Value::Bool(model.reasoning));
    }
    if let Some(service) = state.conversations.as_ref() {
        // One snapshot. Unbound storage is off and has no row. A read that
        // fails is not that: the view would otherwise show the preference off
        // and drop auto-resuming while the durable row is still open, or
        // show them from two moments.
        let id =
            ConversationId::new(&conversation_id).map_err(|_| ConversationError::InvalidInput)?;
        let snapshot = service.usage_limit_snapshot(&id).await?;
        fields.insert("usageLimitAutoContinue".into(), json!(snapshot.enabled));
        if let Some(open) = snapshot.open {
            if let Some(messages) = fields.get_mut("messages").and_then(Value::as_array_mut) {
                for message in messages {
                    let Some(object) = message.as_object_mut() else {
                        continue;
                    };
                    if object.get("executionId").and_then(Value::as_str)
                        != Some(open.causal_execution.as_str())
                    {
                        continue;
                    }
                    if let Some(limit) = object.get_mut("usageLimit").and_then(Value::as_object_mut)
                    {
                        limit.insert("autoResuming".into(), json!(true));
                    }
                }
            }
        }
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

/// The list cut, newest first, to what fits one response frame, and marked
/// incomplete when anything was cut: the caller walks `conversation.observe`
/// for the rest, as for any incomplete list. Five hundred rows are more than
/// one frame holds, and a response past the frame closes the socket, so an
/// uncut list of a few hundred conversations would take the caller's socket
/// with it (`a_list_longer_than_a_frame_is_cut_and_says_so`). A list
/// subscription's frame is cut the same way (`subscription::target::fitted`).
/// Compact JSON is the sum of its parts, so each row is measured once.
fn fitted_list(mut list: ConversationListResult, request_id: &str) -> ConversationListResult {
    let empty = ConversationListResult {
        conversations: Vec::new(),
        complete: false,
    };
    let Ok(empty) = success(request_id, &empty).to_wire_text() else {
        return list;
    };
    let budget = MAX_PAYLOAD_BYTES as usize;
    let mut size = empty.len();
    let mut keep = 0;
    for (index, row) in list.conversations.iter().enumerate() {
        let Ok(row) = serde_json::to_string(row) else {
            break;
        };
        // A comma before every row but the first.
        size += row.len() + usize::from(index > 0);
        if size > budget {
            break;
        }
        keep += 1;
    }
    if keep < list.conversations.len() {
        list.conversations.truncate(keep);
        list.complete = false;
    }
    list
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
                channel_id: entry.channel_id,
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
                channel_id: entry.channel_id,
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
mod permission_caller {
    use super::{permission_caller_for_option, RulePrincipal};
    use nessa_sdk::application::agent_execution::permissions::{PermissionCaller, REMEMBER_ALLOW};

    #[test]
    fn an_ordinary_answer_does_not_need_the_membership_read() {
        assert_eq!(
            permission_caller_for_option("allow", Err("temporarily_unavailable")),
            Ok(PermissionCaller::Grant)
        );
    }

    #[test]
    fn a_remember_answer_still_needs_the_membership_read() {
        assert_eq!(
            permission_caller_for_option(REMEMBER_ALLOW, Err("temporarily_unavailable")),
            Err("temporarily_unavailable")
        );
        assert_eq!(
            permission_caller_for_option(REMEMBER_ALLOW, Ok(RulePrincipal::Owner)),
            Ok(PermissionCaller::Owner)
        );
        assert_eq!(
            permission_caller_for_option(REMEMBER_ALLOW, Ok(RulePrincipal::Grant)),
            Ok(PermissionCaller::Grant)
        );
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
#[cfg(test)]
#[path = "../../tests/conversation/wire_model_changes.rs"]
mod wire_model_changes;
