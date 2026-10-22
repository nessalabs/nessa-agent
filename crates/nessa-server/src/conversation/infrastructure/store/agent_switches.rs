//! Durable switches to another agent and the atomic commit of the agent and
//! model they leave the conversation on.
use super::{
    failed, model_changes, next_revision, read, read_mode_request, stored_time, time, unreadable,
    LocalConversationStore,
};
use crate::conversation::application::{
    ConversationAgentSwitch, ConversationAgentSwitchState, ConversationError, ConversationFuture,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use nessa_local_database::OpenError;
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationId, ConversationModelId};

/// Opening refuses a file whose `agent_switches` is not the current shape.
pub(super) fn validate_shape(connection: &Connection) -> Result<(), OpenError> {
    connection
        .prepare("SELECT conversation_id,request_id,initiator,surface,prior_agent,prior_model,requested_agent,requested_model,state,requested_at_ms FROM agent_switches LIMIT 0")
        .map_err(|error| OpenError::Damaged(format!("conversation metadata lacks the current agent-switch shape: {error}")))?;
    let pending_index: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_index_list('agent_switches') WHERE name='one_pending_agent_switch' AND \"unique\"=1 AND partial=1)",
            [],
            |row| row.get(0),
        )
        .map_err(OpenError::Database)?;
    if !pending_index {
        return Err(OpenError::Damaged(
            "conversation metadata lacks the current pending-switch constraint".into(),
        ));
    }
    Ok(())
}

/// The switch `request` names, or the pending one when it names none.
pub(super) fn read_switch(
    connection: &Connection,
    id: &ConversationId,
    request: Option<&str>,
) -> Result<Option<ConversationAgentSwitch>, ConversationError> {
    let query = "SELECT s.request_id,s.initiator,s.surface,s.prior_agent,s.prior_model,s.requested_agent,s.requested_model,s.state,s.requested_at_ms,c.organization FROM agent_switches s JOIN conversations c ON c.id=s.conversation_id WHERE s.conversation_id=?1 AND ((?2 IS NULL AND s.state='pending') OR s.request_id=?2)";
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
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .optional()
        .map_err(failed)?
        .map(
            |(
                request_id,
                initiator,
                surface,
                prior_agent,
                prior_model,
                requested_agent,
                requested_model,
                state,
                requested_at,
                organization,
            )| {
                let invalid = || unreadable("agent_switches", &id.to_string());
                Ok(ConversationAgentSwitch {
                    conversation_id: id.clone(),
                    organization_id: OrganizationId::new(organization).map_err(|_| invalid())?,
                    request_id,
                    initiator_principal_id: PrincipalId::new(initiator).map_err(|_| invalid())?,
                    initiator_surface_id: surface,
                    prior_agent: AgentId::parse(&prior_agent).ok_or_else(invalid)?,
                    prior_model: ConversationModelId::new(prior_model).map_err(|_| invalid())?,
                    requested_agent: AgentId::parse(&requested_agent).ok_or_else(invalid)?,
                    requested_model: ConversationModelId::new(requested_model)
                        .map_err(|_| invalid())?,
                    state: match state.as_str() {
                        "pending" => ConversationAgentSwitchState::Pending,
                        "applied" => ConversationAgentSwitchState::Applied,
                        "not_applied" => ConversationAgentSwitchState::NotApplied,
                        _ => return Err(invalid()),
                    },
                    requested_at_ms: time(requested_at).ok_or_else(invalid)?,
                })
            },
        )
        .transpose()
}

pub(super) fn begin(
    store: &LocalConversationStore,
    request: ConversationAgentSwitch,
) -> ConversationFuture<'_, ConversationAgentSwitch> {
    store.run(move |connection| {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let id = &request.conversation_id;
        let current = read(&transaction, id)?.ok_or(ConversationError::NotFound)?;
        if current.deletion().is_some() {
            return Err(ConversationError::Deleted);
        }
        if let Some(existing) = read_switch(&transaction, id, Some(&request.request_id))? {
            if !existing.same_request(&request) {
                return Err(ConversationError::RequestConflict);
            }
            return Ok(existing);
        }
        if request.state != ConversationAgentSwitchState::Pending
            || current.organization() != &request.organization_id
            || current.agent() != Some(request.prior_agent)
            || current.model() != &request.prior_model
            || request.prior_agent == request.requested_agent
        {
            return Err(ConversationError::RequestConflict);
        }
        if read_switch(&transaction, id, None)?.is_some()
            || model_changes::read_request(&transaction, id, None)?.is_some()
            || read_mode_request(&transaction, id, None)?.is_some()
        {
            return Err(ConversationError::RequestConflict);
        }
        transaction
            .execute(
                "INSERT INTO agent_switches(conversation_id,request_id,initiator,surface,prior_agent,prior_model,requested_agent,requested_model,state,requested_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9)",
                params![
                    id.to_string(),
                    request.request_id,
                    request.initiator_principal_id.as_str(),
                    request.initiator_surface_id,
                    request.prior_agent.name(),
                    request.prior_model.as_str(),
                    request.requested_agent.name(),
                    request.requested_model.as_str(),
                    stored_time(request.requested_at_ms)?,
                ],
            )
            .map_err(failed)?;
        let saved = read_switch(&transaction, id, Some(&request.request_id))?
            .ok_or(ConversationError::Metadata)?;
        if saved != request {
            return Err(ConversationError::Metadata);
        }
        transaction.commit().map_err(failed)?;
        Ok(saved)
    })
}

pub(super) fn finish<'a>(
    store: &'a LocalConversationStore,
    id: &ConversationId,
    request_id: &str,
    state: ConversationAgentSwitchState,
) -> ConversationFuture<'a, ConversationAgentSwitch> {
    let id = id.clone();
    let request_id = request_id.to_owned();
    let changes = store.changes.clone();
    store.run(move |connection| {
        if state == ConversationAgentSwitchState::Pending {
            return Err(ConversationError::InvalidInput);
        }
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let request =
            read_switch(&transaction, &id, Some(&request_id))?.ok_or(ConversationError::NotFound)?;
        if request.state != ConversationAgentSwitchState::Pending {
            return if request.state == state {
                Ok(request)
            } else {
                Err(ConversationError::RequestConflict)
            };
        }
        let current = read(&transaction, &id)?.ok_or(ConversationError::NotFound)?;
        if state == ConversationAgentSwitchState::Applied {
            if current.agent() != Some(request.prior_agent)
                || current.model() != &request.prior_model
            {
                return Err(ConversationError::Metadata);
            }
            let revision = next_revision(&transaction, &current, false)?;
            transaction
                .execute(
                    "UPDATE conversations SET agent=?1,model=?2,change_revision=?3 WHERE id=?4 AND agent=?5 AND model=?6",
                    params![
                        request.requested_agent.name(),
                        request.requested_model.as_str(),
                        revision,
                        id.to_string(),
                        request.prior_agent.name(),
                        request.prior_model.as_str(),
                    ],
                )
                .map_err(failed)?;
        }
        let terminal = match state {
            ConversationAgentSwitchState::Applied => "applied",
            ConversationAgentSwitchState::NotApplied => "not_applied",
            ConversationAgentSwitchState::Pending => unreachable!(),
        };
        transaction
            .execute(
                "UPDATE agent_switches SET state=?1 WHERE conversation_id=?2 AND request_id=?3 AND state='pending'",
                params![terminal, id.to_string(), request_id],
            )
            .map_err(failed)?;
        transaction.commit().map_err(failed)?;
        if state == ConversationAgentSwitchState::Applied {
            changes.publish(&(current.organization().clone(), current.owner().clone()));
        }
        Ok(ConversationAgentSwitch { state, ..request })
    })
}
