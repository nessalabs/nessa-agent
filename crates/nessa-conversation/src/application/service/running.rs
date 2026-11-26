//! The conversation open here now, and the turn it is running.
use super::{ConversationService, LiveConversation};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    permissions::ActionContext,
    sessions::{CurrentLease, InvocationRecord, SessionSnapshot},
};
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, Initiation, InvocationStage};
use nessa_sdk::domain::agent_execution::leases::{LeaseId, LeasePhase};
use std::sync::Arc;

impl ConversationService {
    /// Conversation `id`, open here now; `None` when it is not.
    pub(super) async fn live(&self, id: &ConversationId) -> Option<Arc<LiveConversation>> {
        let slot = self.inner.conversations.lock().await.get(id).cloned()?;
        slot.value.get()?.as_ref().ok().cloned()
    }
}

/// The conversation's Live agent lease, the running turn, the person it is
/// for with `call` as their request, and who started it; `None` with no
/// turn running or no Live lease to run under.
pub(super) fn running_turn(
    snapshot: &SessionSnapshot,
    call: &str,
) -> Option<(LeaseId, ExecutionId, ActionContext, Initiation)> {
    let parent = snapshot
        .lease
        .as_ref()
        .and_then(CurrentLease::held)
        .filter(|lease| lease.phase() == LeasePhase::Live)?
        .id()
        .clone();
    let turn = running_invocation(snapshot)?;
    let actor = &turn.actor;
    let actor = ActionContext::new(actor.principal_id(), actor.surface_id(), call).ok()?;
    Some((
        parent,
        turn.request.execution_id.clone(),
        actor,
        turn.initiation.clone(),
    ))
}

/// The conversation's running turn, if one is.
fn running_invocation(snapshot: &SessionSnapshot) -> Option<&InvocationRecord> {
    snapshot.invocations.iter().rev().find(|record| {
        record.result.is_none()
            && record
                .scheduling
                .last()
                .is_some_and(|event| event.stage == InvocationStage::Running)
    })
}
