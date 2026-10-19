//! Agents that sit idle with nobody watching are stopped, and their
//! conversations stay open (issue #998, `docs/design/agent-parking.md`).
//!
//! ```text
//! every few seconds, for each live conversation:
//!   busy, followed, or an app call waiting ──▶ touched now
//!   untouched for park_after ──▶ take the submission lock
//!     ──▶ the same checks again ──▶ suspend (lease ends idle) ──▶ slot released
//! the next message ──▶ opens the conversation ──▶ session/resume
//! ```
//!
//! Parking is the suspend an agent that stopped itself already gets: what it
//! owes stays owed and its provider session id stays recorded. The submission
//! lock is held until the slot is released, so a message that arrives meanwhile
//! waits and then opens the conversation again rather than being refused.
use super::{ConversationService, LiveConversation, Slot};
use futures_util::{stream, StreamExt};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    agents::AttachmentPhase, permissions::ActionContext,
};
use nessa_sdk::domain::agent_execution::leases::LeaseEndCause;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use uuid::Uuid;

use super::super::{
    environment::AgentEnd, mcp_apps::McpAppInitiator, metadata_target::load_conversation,
};

/// The longest wait between two looks at the live conversations. A short
/// idle limit is looked at four times within it instead.
const MOST_BETWEEN_SWEEPS: Duration = Duration::from_secs(15);

/// Agents parked at once. Each park saves its lease's end through the one
/// store, as a shutdown's stops do, so they go a few at a time.
const PARKS_AT_ONCE: usize = 64;

impl ConversationService {
    /// Park idle agents nobody watches until the gateway retires. Started
    /// once by composition; does nothing when `park_after` is `None`.
    pub async fn run_parking(&self) {
        let Some(park_after) = self.inner.limits.park_after else {
            return;
        };
        let between = (park_after / 4).clamp(Duration::from_millis(100), MOST_BETWEEN_SWEEPS);
        loop {
            tokio::select! {
                () = tokio::time::sleep(between) => {}
                () = self.retired() => return,
            }
            if self.inner.retirement.get().is_some() {
                return;
            }
            self.park_idle(park_after).await;
        }
    }

    /// One look at every live conversation: touch those in use, and park
    /// those untouched for `park_after`. Returns how many it parked.
    pub(super) async fn park_idle(&self, park_after: Duration) -> usize {
        let owners: Vec<(ConversationId, Arc<Slot>)> = self
            .inner
            .conversations
            .lock()
            .await
            .iter()
            .map(|(id, slot)| (id.clone(), slot.clone()))
            .collect();
        let now = self.inner.clock.unix_milliseconds();
        let mut due = Vec::new();
        for (id, slot) in owners {
            let Some(live) = settled(&slot) else {
                continue;
            };
            if !self.parkable(&id, &live).await {
                slot.touch(now);
            } else if slot.untouched_for(now) >= park_after {
                due.push((id, slot));
            }
        }
        stream::iter(due)
            .map(|(id, slot)| async move { self.park(&id, slot, park_after).await })
            .buffer_unordered(PARKS_AT_ONCE)
            .filter(|parked| std::future::ready(*parked))
            .count()
            .await
    }

    /// Whether `live` could be parked now: attached, able to resume its
    /// provider session, idle, nobody following it, and no app call of it
    /// waiting on a review. An agent that cannot resume would lose its
    /// session's context, so it is never parked. A permission review or a
    /// question belongs to a running turn, so its agent is not idle.
    async fn parkable(&self, id: &ConversationId, live: &LiveConversation) -> bool {
        live.agent.attachment_status().phase() == AttachmentPhase::Attached
            && live.agent.operation_capabilities().session_resume()
            && !self.inner.live_changes.followed(id)
            && live.app_reviews.entries().is_empty()
            && live.agent.is_idle().await
    }

    /// Park `slot` if, under the conversation's submission lock, it is still
    /// the live owner, untouched for `park_after` and parkable. Returns
    /// whether it was parked.
    async fn park(&self, id: &ConversationId, slot: Arc<Slot>, park_after: Duration) -> bool {
        // Retirement waits for this to finish, or this does not start.
        let Ok(_admission) = self.admit().await else {
            return false;
        };
        // Held until the slot is released: what arrives meanwhile waits for
        // it, and opens the conversation again once it is gone.
        let _submissions = self.inner.mode_changes.lock(id).await;
        let owner = self.inner.conversations.lock().await.get(id).cloned();
        if !owner.is_some_and(|owner| Arc::ptr_eq(&owner, &slot)) {
            return false;
        }
        let Some(live) = settled(&slot) else {
            return false;
        };
        let now = self.inner.clock.unix_milliseconds();
        if slot.untouched_for(now) < park_after || !self.parkable(id, &live).await {
            return false;
        }
        // A delete lets go of the lock once its tombstone is written, and
        // stops the agent itself: its lease ends as the delete's, not idle.
        match load_conversation(self.inner.metadata.as_ref(), id).await {
            Ok(Some(record)) if record.deletion().is_none() => {}
            _ => return false,
        }
        drop(live);
        let Ok(actor) =
            ActionContext::new("gateway", "agent_idle", format!("idle-{}", Uuid::new_v4()))
        else {
            return false;
        };
        tracing::info!(conversation_id = %id, idle_seconds = park_after.as_secs(), "parking an idle agent");
        match self
            .stop_and_release(
                id,
                slot,
                &actor,
                &McpAppInitiator::System,
                LeaseEndCause::Idle,
                AgentEnd::Suspend,
            )
            .await
        {
            Ok(()) => true,
            Err(stop) => {
                tracing::warn!(conversation_id = %id, ?stop, "an idle agent's stop did not confirm cleanup");
                false
            }
        }
    }
}

/// The live conversation `slot` holds, when it opened and nothing is
/// stopping it.
fn settled(slot: &Slot) -> Option<Arc<LiveConversation>> {
    if slot.stopping.load(Ordering::SeqCst) {
        return None;
    }
    slot.value.get()?.as_ref().ok().cloned()
}
