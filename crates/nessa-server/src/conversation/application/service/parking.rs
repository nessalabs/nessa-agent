//! Agents that sit idle with nobody watching are stopped, and their
//! conversations stay open (issue #998, `docs/design/agent-parking.md`).
//! One in a cloud workspace of its own is stopped once idle for
//! `workspace_idle`, watched or not, unless a background command of it still
//! runs; its VM then stops too, keeping its disk, and the next message
//! resumes both.
//!
//! ```text
//! every few seconds, for each live conversation:
//!   busy, or an app call waiting ──▶ touched now
//!   in a cloud workspace: a background command running ──▶ touched now
//!   elsewhere: followed ──▶ touched now
//!   untouched for its limit (workspace_idle, or park_after) ──▶ take the submission lock
//!     ──▶ the same checks again ──▶ suspend (lease ends idle) ──▶ slot released
//!     ──▶ in a cloud workspace: its connection let go, its VM stopped
//! the next message ──▶ opens the conversation (resuming its VM) ──▶ session/resume
//! an opening past the bound on live agents ──▶ the parkable one touched
//!   longest ago is parked now (`make_room`)
//! ```
//!
//! Parking is the suspend an agent that stopped itself already gets: what it
//! owes stays owed and its provider session id stays recorded. The submission
//! lock is held until the slot is released, so a message that arrives meanwhile
//! waits and then opens the conversation again rather than being refused.
use super::{ConversationError, ConversationService, LiveConversation, Slot};
use futures_util::{stream, StreamExt};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    agents::AttachmentPhase, permissions::ActionContext,
};
use nessa_sdk::domain::agent_execution::leases::LeaseEndCause;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use tokio::sync::OwnedMutexGuard;
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
    /// `id`'s agent closed its own thread (`close_thread`): once its turn
    /// ends it is parked, watched or not, and a thread in a cloud workspace
    /// stops its VM with it. The next message opens it again.
    ///
    /// # Errors
    /// [`ConversationError::Unavailable`] when it is not open now.
    pub async fn close_after_turn(&self, id: &ConversationId) -> Result<(), ConversationError> {
        let slot = self
            .inner
            .conversations
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or(ConversationError::Unavailable)?;
        slot.closing.store(true, Ordering::SeqCst);
        tracing::info!(conversation_id = %id, "the agent closed its thread; it stops once its turn ends");
        Ok(())
    }

    /// Park idle agents until the gateway retires: those nobody watches
    /// after `park_after`, when it is set, and those in a cloud workspace
    /// after `workspace_idle`, and those whose agent closed its thread at
    /// once. Started once by composition.
    pub async fn run_parking(&self) {
        let park_after = self.inner.limits.park_after;
        let shortest = park_after.map_or(self.inner.limits.workspace_idle, |park_after| {
            park_after.min(self.inner.limits.workspace_idle)
        });
        let between = (shortest / 4).clamp(Duration::from_millis(100), MOST_BETWEEN_SWEEPS);
        loop {
            tokio::select! {
                () = tokio::time::sleep(between) => {}
                () = self.retired() => return,
            }
            if self.inner.retirement.get().is_some() {
                return;
            }
            self.sweep(park_after).await;
        }
    }

    /// One look at every live conversation, parking those nobody watches
    /// that sat untouched for `park_after`, and those in a cloud workspace
    /// for `workspace_idle`. Returns how many it parked.
    pub(super) async fn park_idle(&self, park_after: Duration) -> usize {
        self.sweep(Some(park_after)).await
    }

    /// One look at every live conversation: touch those in use, and park
    /// those untouched for their limit ([`Self::idle_limit`]). Returns how
    /// many it parked.
    pub(super) async fn sweep(&self, park_after: Option<Duration>) -> usize {
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
            match self.idle_limit(&id, &slot, &live, park_after).await {
                Idle::InUse => slot.touch(now),
                Idle::Never => {}
                Idle::After(limit) => {
                    if slot.untouched_for(now) >= limit {
                        due.push((id, slot, limit));
                    }
                }
            }
        }
        stream::iter(due)
            .map(|(id, slot, limit)| async move { self.park(&id, slot, limit).await })
            .buffer_unordered(PARKS_AT_ONCE)
            .filter(|parked| std::future::ready(*parked))
            .count()
            .await
    }

    /// How long `live` must sit untouched to be parked by a sweep under
    /// `park_after`. An agent that is attached, able to resume its provider
    /// session, idle and with no app call waiting on a review may be parked;
    /// one that cannot resume would lose its session's context, and a
    /// permission review or a question belongs to a running turn. In a cloud
    /// workspace of its own it waits `workspace_idle`, watched or not, while
    /// no background command of it runs; elsewhere it waits `park_after`
    /// while nobody follows it.
    async fn idle_limit(
        &self,
        id: &ConversationId,
        slot: &Slot,
        live: &LiveConversation,
        park_after: Option<Duration>,
    ) -> Idle {
        if !self.could_park(live).await {
            return Idle::InUse;
        }
        // Its agent closed its thread: parked now that it is idle.
        if slot.closing.load(Ordering::SeqCst) {
            return Idle::After(Duration::ZERO);
        }
        if self.inner.environment.in_workspace(id).await {
            let running = self
                .inner
                .background_tasks
                .get()
                .is_some_and(|tasks| tasks.running_for(id));
            return if running {
                Idle::InUse
            } else {
                Idle::After(self.inner.limits.workspace_idle)
            };
        }
        if self.inner.live_changes.followed(id) {
            return Idle::InUse;
        }
        park_after.map_or(Idle::Never, Idle::After)
    }

    /// Whether `live` is in a state an agent may be parked in, whoever
    /// watches it.
    async fn could_park(&self, live: &LiveConversation) -> bool {
        live.agent.attachment_status().phase() == AttachmentPhase::Attached
            && live.agent.operation_capabilities().session_resume()
            && live.app_reviews.entries().is_empty()
            && live.agent.is_idle().await
    }

    /// Whether `live` could be parked now to make room: as a sweep would,
    /// and with nobody following it.
    async fn parkable(&self, id: &ConversationId, live: &LiveConversation) -> bool {
        self.could_park(live).await && !self.inner.live_changes.followed(id)
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
        let submissions = self.inner.mode_changes.lock(id).await;
        self.park_held(id, slot, ParkWhen::IdleFor(park_after), submissions)
            .await
    }

    /// Park `slot` with its conversation's submission lock held, if it is
    /// still the live owner, parkable, and unused as `when` asks. Returns
    /// whether it was parked.
    pub(super) async fn park_held(
        &self,
        id: &ConversationId,
        slot: Arc<Slot>,
        when: ParkWhen,
        _submissions: OwnedMutexGuard<()>,
    ) -> bool {
        let owner = self.inner.conversations.lock().await.get(id).cloned();
        if !owner.is_some_and(|owner| Arc::ptr_eq(&owner, &slot)) {
            return false;
        }
        let Some(live) = settled(&slot) else {
            return false;
        };
        let now = self.inner.clock.unix_milliseconds();
        let parkable = match when {
            // The same limit again: its placement never changes, but whether
            // it is watched, busy or running something may have.
            ParkWhen::IdleFor(limit) => {
                slot.untouched_for(now) >= limit
                    && matches!(
                        self.idle_limit(id, &slot, &live, Some(limit)).await,
                        Idle::After(again) if slot.untouched_for(now) >= again
                    )
            }
            ParkWhen::UnusedSince(touched) => {
                slot.touched.load(Ordering::SeqCst) <= touched && self.parkable(id, &live).await
            }
        };
        if !parkable {
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
        match when {
            ParkWhen::IdleFor(park_after) => {
                tracing::info!(conversation_id = %id, idle_seconds = park_after.as_secs(), "parking an idle agent")
            }
            ParkWhen::UnusedSince(_) => {
                tracing::info!(conversation_id = %id, "parking an idle agent to make room")
            }
        }
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

/// How long a live conversation must sit untouched before a sweep parks it.
enum Idle {
    /// In use now: touched.
    InUse,
    /// Never parked by a sweep: nobody watches it, and idle agents here are
    /// not parked.
    Never,
    /// Parked once untouched this long.
    After(Duration),
}

/// How long a conversation must have gone unused for [`ConversationService::park_held`]
/// to park it.
pub(super) enum ParkWhen {
    /// Untouched this long: the idle sweep.
    IdleFor(Duration),
    /// Untouched since it was picked, at this touch: past the bound on live
    /// agents, the one used longest ago, however recently
    /// ([`ConversationService::make_room`]). A read or message that lands
    /// between the pick and the lock makes it used, and the next is tried.
    UnusedSince(u64),
}

/// The live conversation `slot` holds, when it opened and nothing is
/// stopping it.
fn settled(slot: &Slot) -> Option<Arc<LiveConversation>> {
    if slot.stopping.load(Ordering::SeqCst) {
        return None;
    }
    slot.value.get()?.as_ref().ok().cloned()
}
