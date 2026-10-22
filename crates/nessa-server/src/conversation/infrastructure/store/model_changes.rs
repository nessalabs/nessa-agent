//! Durable native-model intents and atomic committed selections.
use super::{
    failed, next_revision, read, read_mode_request, stored_time, time, unreadable,
    LocalConversationStore,
};
use crate::conversation::application::{
    ConversationError, ConversationFuture, ConversationModelApplication, ConversationModelRequest,
    ConversationModelRequestState, ConversationModelSelection,
};
use crate::conversation::domain::Conversation;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use nessa_local_database::OpenError;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_protocol::conversation::view::ConversationModelChangeEffect;
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;

/// Current contract only: opening refuses an earlier alpha shape before any
/// service can issue SQL against it. A version 7 file may already have held
/// `model_requests` when it was brought to 8 (`schema_upgrades/7.sql`); this
/// checks that the table it kept is the current one.
pub(super) fn validate_shape(connection: &Connection) -> Result<(), OpenError> {
    connection.prepare("SELECT conversation_id,request_id,initiator,surface,prior_provider,prior_context,requested_provider,requested_context,prior_model,prior_mode,requested_model,state,application,applied_mode,effect,requested_at_ms FROM model_requests LIMIT 0")
        .map_err(|error| OpenError::Damaged(format!("conversation metadata lacks the required current model-request shape: {error}")))?;
    let pending_index: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_index_list('model_requests') WHERE name='one_pending_model_request' AND \"unique\"=1 AND partial=1)",
        [], |row|row.get(0),
    ).map_err(OpenError::Database)?;
    if !pending_index {
        return Err(OpenError::Damaged(
            "conversation metadata lacks the current pending-model constraint".into(),
        ));
    }
    Ok(())
}

pub(super) fn read_request(
    connection: &Connection,
    id: &ConversationId,
    request: Option<&str>,
) -> Result<Option<ConversationModelRequest>, ConversationError> {
    let query = "SELECT m.request_id,m.initiator,m.surface,m.prior_model,m.prior_mode,m.requested_model,m.state,m.requested_at_ms,m.application,m.applied_mode,m.effect,c.organization,m.prior_provider,m.prior_context,m.requested_provider,m.requested_context FROM model_requests m JOIN conversations c ON c.id=m.conversation_id WHERE m.conversation_id=?1 AND ((?2 IS NULL AND m.state='pending') OR m.request_id=?2)";
    connection
        .query_row(query, params![id.to_string(), request], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, String>(15)?,
            ))
        })
        .optional()
        .map_err(failed)?
        .map(
            |(
                request_id,
                initiator,
                surface,
                prior,
                prior_mode,
                requested,
                state,
                requested_at,
                application,
                applied_mode,
                effect,
                organization,
                prior_provider,
                prior_context,
                requested_provider,
                requested_context,
            )| {
                let invalid = || unreadable("model_requests", &id.to_string());
                let model = ConversationModelId::new(requested).map_err(|_| invalid())?;
                let prior_model = ConversationModelId::new(prior).map_err(|_| invalid())?;
                let prior_identity =
                    ProviderIdentity::new(prior_provider, prior_model.as_str(), prior_context)
                        .map_err(|_| invalid())?;
                let requested_identity =
                    ProviderIdentity::new(requested_provider, model.as_str(), requested_context)
                        .map_err(|_| invalid())?;
                if prior_identity.name() != requested_identity.name() {
                    return Err(invalid());
                }
                let application = match (
                    application.as_deref(),
                    applied_mode.as_deref(),
                    effect.as_deref(),
                ) {
                    (None, None, None) => None,
                    (Some("refused"), None, None) => Some(ConversationModelApplication::Refused),
                    (Some("uncertain"), None, None) => {
                        Some(ConversationModelApplication::Uncertain)
                    }
                    (Some("applied"), Some(mode), Some(effect)) => Some(
                        ConversationModelApplication::Applied(ConversationModelSelection {
                            model: model.clone(),
                            approval_mode: ConversationApprovalMode::parse(mode)
                                .ok_or_else(invalid)?,
                            effect: match effect {
                                "subsequent_responses" => {
                                    ConversationModelChangeEffect::SubsequentResponses
                                }
                                "next_turn" => ConversationModelChangeEffect::NextTurn,
                                _ => return Err(invalid()),
                            },
                        }),
                    ),
                    _ => return Err(invalid()),
                };
                let state = match state.as_str() {
                    "pending" => ConversationModelRequestState::Pending,
                    "applied" => ConversationModelRequestState::Applied,
                    "not_applied" => ConversationModelRequestState::NotApplied,
                    _ => return Err(invalid()),
                };
                if request_id.is_empty()
                    || request_id.len() > Conversation::MAX_CREATOR_CONTEXT_BYTES
                    || surface.is_empty()
                    || surface.len() > Conversation::MAX_CREATOR_CONTEXT_BYTES
                    || (state != ConversationModelRequestState::Pending && application.is_none())
                    || (state == ConversationModelRequestState::Applied
                        && !matches!(application, Some(ConversationModelApplication::Applied(_))))
                    || (state == ConversationModelRequestState::NotApplied
                        && !matches!(application, Some(ConversationModelApplication::Refused)))
                {
                    return Err(invalid());
                }
                Ok(ConversationModelRequest {
                    conversation_id: id.clone(),
                    organization_id: OrganizationId::new(organization).map_err(|_| invalid())?,
                    request_id,
                    initiator_principal_id: PrincipalId::new(initiator).map_err(|_| invalid())?,
                    initiator_surface_id: surface,
                    prior_identity,
                    requested_identity,
                    prior_model,
                    prior_approval_mode: ConversationApprovalMode::parse(&prior_mode)
                        .ok_or_else(invalid)?,
                    requested_model: model,
                    state,
                    application,
                    requested_at_ms: time(requested_at).ok_or_else(invalid)?,
                })
            },
        )
        .transpose()
}

pub(super) fn begin(
    store: &LocalConversationStore,
    request: ConversationModelRequest,
) -> ConversationFuture<'_, ConversationModelRequest> {
    store.run(move |connection| {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failed)?;
        let current = read(&transaction, &request.conversation_id)?.ok_or(ConversationError::NotFound)?;
        if current.deletion().is_some() { return Err(ConversationError::Deleted); }
        if let Some(existing) = read_request(&transaction, &request.conversation_id, Some(&request.request_id))? {
            if existing.organization_id != request.organization_id
                || existing.initiator_principal_id != request.initiator_principal_id
                || existing.initiator_surface_id != request.initiator_surface_id
                || existing.requested_model != request.requested_model
                || existing.requested_identity != request.requested_identity {
                return Err(ConversationError::RequestConflict);
            }
            return Ok(existing);
        }
        if read_request(&transaction, &request.conversation_id, None)?.is_some()
            || read_mode_request(&transaction, &request.conversation_id, None)?.is_some()
            || super::agent_switches::read_switch(&transaction, &request.conversation_id, None)?.is_some()
            || current.model() != &request.prior_model
            || current.approval_mode() != request.prior_approval_mode
            || current.organization() != &request.organization_id
            || request.state != ConversationModelRequestState::Pending
            || request.application.is_some()
            || request.prior_identity.model_id()!=request.prior_model.as_str()
            || request.requested_identity.model_id()!=request.requested_model.as_str()
            || request.prior_identity.name()!=request.requested_identity.name() {
            return Err(ConversationError::ModelChangeUncertain);
        }
        let insert = "INSERT INTO model_requests(conversation_id,request_id,initiator,surface,prior_model,prior_mode,requested_model,state,requested_at_ms,prior_provider,prior_context,requested_provider,requested_context) VALUES(?1,?2,?3,?4,?5,?6,?7,'pending',?8,?9,?10,?11,?12)";
        transaction.execute(insert, params![
            request.conversation_id.to_string(), request.request_id,
            request.initiator_principal_id.as_str(), request.initiator_surface_id,
            request.prior_model.as_str(), request.prior_approval_mode.as_str(),
            request.requested_model.as_str(), stored_time(request.requested_at_ms)?,
            request.prior_identity.name(),request.prior_identity.context(),request.requested_identity.name(),request.requested_identity.context(),
        ]).map_err(failed)?;
        let saved = read_request(&transaction, &request.conversation_id, Some(&request.request_id))?
            .ok_or(ConversationError::Metadata)?;
        if saved != request { return Err(ConversationError::Metadata); }
        transaction.commit().map_err(failed)?;
        Ok(saved)
    })
}

pub(super) fn observe<'a>(
    store: &'a LocalConversationStore,
    id: &ConversationId,
    request_id: &str,
    application: ConversationModelApplication,
) -> ConversationFuture<'a, ConversationModelRequest> {
    let id = id.clone();
    let request_id = request_id.to_owned();
    store.run(move |connection| {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failed)?;
        let request = read_request(&transaction, &id, Some(&request_id))?.ok_or(ConversationError::NotFound)?;
        if request.state != ConversationModelRequestState::Pending { return Err(ConversationError::RequestConflict); }
        if let Some(existing) = &request.application {
            if existing == &application { return Ok(request); }
            // Only a verified SDK semantic record may refine uncertainty into
            // an acknowledged selection. Applied or refused facts do not change.
            if !matches!((existing, &application), (ConversationModelApplication::Uncertain, ConversationModelApplication::Applied(_) | ConversationModelApplication::Refused)) {
                return Err(ConversationError::RequestConflict);
            }
        }
        let (state, mode, effect) = match &application {
            ConversationModelApplication::Applied(selection) => {
                if selection.model != request.requested_model { return Err(ConversationError::RequestConflict); }
                let effect = match selection.effect {
                    ConversationModelChangeEffect::SubsequentResponses => "subsequent_responses",
                    ConversationModelChangeEffect::NextTurn => "next_turn",
                };
                ("applied", Some(selection.approval_mode.as_str()), Some(effect))
            }
            ConversationModelApplication::Refused => ("refused", None, None),
            ConversationModelApplication::Uncertain => ("uncertain", None, None),
        };
        let update = "UPDATE model_requests SET application=?1,applied_mode=?2,effect=?3 WHERE conversation_id=?4 AND request_id=?5 AND state='pending'";
        transaction.execute(update, params![state, mode, effect, id.to_string(), request_id]).map_err(failed)?;
        transaction.commit().map_err(failed)?;
        Ok(ConversationModelRequest { application: Some(application), ..request })
    })
}

pub(super) fn finish<'a>(
    store: &'a LocalConversationStore,
    id: &ConversationId,
    request_id: &str,
    state: ConversationModelRequestState,
) -> ConversationFuture<'a, ConversationModelRequest> {
    let id = id.clone();
    let request_id = request_id.to_owned();
    let changes = store.changes.clone();
    store.run(move |connection| {
        if state == ConversationModelRequestState::Pending { return Err(ConversationError::InvalidInput); }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failed)?;
        let request = read_request(&transaction, &id, Some(&request_id))?.ok_or(ConversationError::NotFound)?;
        if request.application.is_none()
            || (state == ConversationModelRequestState::Applied && !matches!(request.application, Some(ConversationModelApplication::Applied(_))))
            || (state == ConversationModelRequestState::NotApplied && !matches!(request.application, Some(ConversationModelApplication::Refused))) {
            return Err(ConversationError::ModelChangeUncertain);
        }
        if request.state != ConversationModelRequestState::Pending {
            return if request.state == state { Ok(request) } else { Err(ConversationError::RequestConflict) };
        }
        let current = read(&transaction, &id)?.ok_or(ConversationError::NotFound)?;
        if current.deletion().is_some() { return Err(ConversationError::Deleted); }
        if current.model() != &request.prior_model || current.approval_mode() != request.prior_approval_mode {
            return Err(ConversationError::ModelChangeUncertain);
        }
        if state == ConversationModelRequestState::Applied {
            let Some(ConversationModelApplication::Applied(selection)) = &request.application else {
                return Err(ConversationError::ModelChangeUncertain);
            };
            let revision = next_revision(&transaction, &current, false)?;
            let update = "UPDATE conversations SET model=?1,approval_mode=?2,change_revision=?3 WHERE id=?4 AND model=?5 AND approval_mode=?6";
            transaction.execute(update, params![selection.model.as_str(), selection.approval_mode.as_str(), revision, id.to_string(), request.prior_model.as_str(), request.prior_approval_mode.as_str()]).map_err(failed)?;
        }
        let terminal = match state {
            ConversationModelRequestState::Applied => "applied",
            ConversationModelRequestState::NotApplied => "not_applied",
            ConversationModelRequestState::Pending => unreachable!(),
        };
        transaction.execute("UPDATE model_requests SET state=?1 WHERE conversation_id=?2 AND request_id=?3 AND state='pending'", params![terminal, id.to_string(), request_id]).map_err(failed)?;
        transaction.commit().map_err(failed)?;
        if state == ConversationModelRequestState::Applied {
            changes.publish(&(current.organization().clone(), current.owner().clone()));
        }
        Ok(ConversationModelRequest { state, ..request })
    })
}
