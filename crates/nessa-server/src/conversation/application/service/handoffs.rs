//! Switching a conversation to another agent, and forking one into a new
//! conversation (issue #1080, `docs/design/conversation-handoffs.md`). Both
//! are one SDK record, a handoff, after which the agent runs in a native
//! session that has not seen the turns before it and is given them as text.
//!
//! ```text
//! switchAgent ─▶ submission lock ─▶ idle ─▶ journal: pending
//!   ─▶ the agent it ran on suspended ─▶ opened on the pending switch's agent,
//!      its history told to expect the switch ─▶ attached
//!   ─▶ the history holds the switch? applied (agent and model committed)
//!                                   : not applied (stays on its agent)
//! fork ─▶ the source's committed history, carried ─▶ seed for the new id
//!   ─▶ create on the source's agent, model, preset and environment
//!   ─▶ its opening takes the seed ─▶ the history records the fork first
//! ```
//!
//! The history decides whether a switch happened; the journal follows it.
//! An opening while a switch is pending runs the requested agent, so a
//! gateway that stopped mid-switch carries it on at the next opening, and
//! settles it there.
use super::{
    load_conversation, supervised, ActionContext, AgentEnd, ConversationCaller, ConversationError,
    ConversationId, ConversationService, McpAppInitiator, RequestedAgent, RequestedConversation,
    Slot,
};
use crate::conversation::application::{
    ConversationAgentSwitch, ConversationAgentSwitchState, ScheduleActor, SelectionChange,
};
use crate::conversation::domain::Conversation;
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationModelId, ConversationSummary};
use nessa_sdk::application::agent_execution::{
    commands::{CreationFailure, CreationReceipt, CreationStorage, CreationStorageError},
    sessions::{HandoffIntent, StorageError},
};
use nessa_sdk::domain::agent_execution::leases::LeaseEndCause;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Tells one fork seeding from another of the same request.
static NEXT_SEED: AtomicU64 = AtomicU64::new(0);

/// What one opening of a conversation runs, and what its history is told to
/// expect besides opening.
pub(super) struct OpeningPlan {
    pub(super) agent: AgentId,
    pub(super) model: ConversationModelId,
    pub(super) handoff: Option<HandoffIntent>,
    /// The pending switch this opening carries on, settled once it ends.
    pub(super) switch: Option<ConversationAgentSwitch>,
}

/// A fork waiting for the opening its creation runs: the request that
/// asked for it, so only that request's creation takes it and only that
/// request lets it go.
pub(super) struct ForkSeed {
    /// This seeding's own, so only the call that placed it lets it go.
    token: u64,
    request: String,
    surface: String,
    intent: HandoffIntent,
}

impl ConversationService {
    /// Carry the conversation on under `agent` and `model`, in a new native
    /// session given its earlier turns with the next message. Answers
    /// whether the switch was made; the same request answers as it did.
    pub async fn switch_agent(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        agent: AgentId,
        model: ConversationModelId,
    ) -> Result<bool, ConversationError> {
        let service = self.clone();
        let changed = (self.inner.live_changes.clone(), id.clone());
        supervised(async move {
            let result = service.switch(&id, &caller, agent, model).await;
            changed.0.publish(&changed.1);
            result
        })
        .await
    }

    async fn switch(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        agent: AgentId,
        model: ConversationModelId,
    ) -> Result<bool, ConversationError> {
        let _admission = self.admit().await?;
        // Held to the end: no message is admitted to the agent being left,
        // and none to the new one before its history holds the switch.
        let _submissions = self.inner.mode_changes.lock(id).await;
        let actor = caller.actor()?;
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        let request = ConversationAgentSwitch {
            conversation_id: id.clone(),
            organization_id: caller.organization_id.clone(),
            request_id: caller.action_id.clone(),
            initiator_principal_id: caller.principal_id.clone(),
            initiator_surface_id: caller.surface_id.clone(),
            prior_agent: record.agent().ok_or(ConversationError::AgentUnsupported)?,
            prior_model: record.model().clone(),
            requested_agent: agent,
            requested_model: model,
            state: ConversationAgentSwitchState::Pending,
            requested_at_ms: self.inner.clock.unix_milliseconds(),
        };
        if let Some(saved) = self
            .inner
            .metadata
            .agent_switch(id, &caller.action_id)
            .await?
        {
            if !saved.same_request(&request) {
                return Err(ConversationError::RequestConflict);
            }
            let settled = match saved.state {
                ConversationAgentSwitchState::Pending => {
                    self.carry_switch(id, caller, &actor, &saved).await?
                }
                _ => saved,
            };
            return Ok(settled.state == ConversationAgentSwitchState::Applied);
        }
        // Another request's switch the gateway stopped in the middle of is
        // carried on first, so this one starts from where it left the
        // conversation.
        if let Some(pending) = self.inner.metadata.pending_agent_switch(id).await? {
            self.carry_switch(id, caller, &actor, &pending).await?;
            return Err(ConversationError::RequestConflict);
        }
        // The same agent changes model on its own session (`setModel`).
        if record.agent() == Some(agent) {
            return Err(ConversationError::InvalidInput);
        }
        let place = self.agent_place(id).await?;
        self.inner
            .agents
            .resolve_model(agent, request.requested_model.as_str(), place.as_ref())
            .await?;
        // A turn running or waiting stays with the agent it was given to,
        // and so does one a restart or a failure left owed: it was saved
        // before the switch, so the new session would run it given nothing
        // of what came before. An agent whose opening failed holds no turn,
        // and a switch is how a person leaves an agent that cannot open.
        if self.owes_turns(id).await? {
            return Err(ConversationError::TurnRunning);
        }
        let slot = self.inner.conversations.lock().await.get(id).cloned();
        if let Some(slot) = &slot {
            if let Ok(live) = self.wait_for_slot(id, slot.clone()).await {
                if !live.agent.is_idle().await || !live.app_reviews.entries().is_empty() {
                    return Err(ConversationError::TurnRunning);
                }
            }
        }
        let begun = self.inner.metadata.begin_agent_switch(request).await?;
        if let Some(slot) = slot {
            if let Err(stop) = self
                .stop_and_release(
                    id,
                    slot.clone(),
                    &actor,
                    &McpAppInitiator::System,
                    LeaseEndCause::Stopped,
                    AgentEnd::Suspend,
                )
                .await
            {
                tracing::warn!(conversation_id = %id, ?stop, "the agent being left did not confirm its stop; the switch is not made");
                // Settled as any is: a reading meanwhile may have opened
                // the requested agent, and the history decides.
                self.settle_switch(id, &begun, &actor, Some(&slot)).await?;
                return Err(ConversationError::Unavailable);
            }
        }
        let settled = self.carry_switch(id, caller, &actor, &begun).await?;
        Ok(settled.state == ConversationAgentSwitchState::Applied)
    }

    /// Open the conversation on `pending`'s agent, wait for it to attach, and
    /// settle the switch as its history decided. One that was not made
    /// leaves no agent of the requested provider behind.
    async fn carry_switch(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        actor: &ActionContext,
        pending: &ConversationAgentSwitch,
    ) -> Result<ConversationAgentSwitch, ConversationError> {
        let opened = self.resolve(id, caller).await;
        if let Ok(live) = &opened {
            let _ = live.join_attachment_owner().await;
        }
        let settled = self.settle_switch(id, pending, actor, None).await?;
        if settled.state == ConversationAgentSwitchState::NotApplied {
            opened?;
        }
        Ok(settled)
    }

    /// Settle `pending` once the opening that carried it has ended. Every
    /// settling of a switch comes here, and any number may run at once.
    ///
    /// A history holding the switch settles it applied: a saved switch is
    /// never taken back. One that does not is settled not made only while no
    /// agent of the requested provider runs and none can start: under the
    /// lock an opening takes to place its slot, with no such slot there. Until
    /// then an attach of the requested agent could still save the switch, and
    /// a switch settled not made but saved after would leave the history on
    /// one agent and the conversation on another. So each such slot found is
    /// stopped first, and the question asked again. `prior` is the slot of
    /// the agent being left, when it could not be stopped: while it is still
    /// in place no opening of the requested agent can have run. A switch
    /// another settling already decided answers as it was decided.
    pub(super) async fn settle_switch(
        &self,
        id: &ConversationId,
        pending: &ConversationAgentSwitch,
        actor: &ActionContext,
        prior: Option<&Arc<Slot>>,
    ) -> Result<ConversationAgentSwitch, ConversationError> {
        loop {
            // Read before the lock, which every conversation's opening
            // needs: a saved switch stays saved, so this answer holds.
            if self.holds_switch(id, pending).await? {
                return self.finish_switch(id, pending, true).await;
            }
            let slots = self.inner.conversations.lock().await;
            let journal = self
                .inner
                .metadata
                .agent_switch(id, &pending.request_id)
                .await?
                .ok_or(ConversationError::Metadata)?;
            if journal.state != ConversationAgentSwitchState::Pending {
                return Ok(journal);
            }
            let slot = slots
                .get(id)
                .filter(|slot| !prior.is_some_and(|prior| Arc::ptr_eq(prior, slot)))
                .cloned();
            let Some(slot) = slot else {
                // Nothing runs that could save it, and nothing starts while
                // the lock is held; asked once more for a save that landed
                // since the read above, by a slot gone since. Settled before
                // the lock goes, so an opening after it reads a settled
                // journal and runs the agent it decided.
                let held = self.holds_switch(id, pending).await?;
                let settled = self.finish_switch(id, pending, held).await;
                drop(slots);
                return settled;
            };
            drop(slots);
            // While the switch is pending every opening runs the requested
            // agent, so this slot is one. Stopped by this slot alone: one
            // placed after it is not touched.
            if let Err(stop) = self
                .stop_and_release(
                    id,
                    slot,
                    actor,
                    &McpAppInitiator::System,
                    LeaseEndCause::Stopped,
                    AgentEnd::Suspend,
                )
                .await
            {
                tracing::warn!(conversation_id = %id, ?stop, "an agent of a switch not yet saved did not confirm its stop; the next opening carries the switch on");
                return Err(ConversationError::Unavailable);
            }
        }
    }

    /// Settle `pending` applied when the committed history already holds it;
    /// `None`, leaving it pending, when it does not yet.
    pub(super) async fn settle_if_saved(
        &self,
        id: &ConversationId,
        pending: &ConversationAgentSwitch,
    ) -> Result<Option<ConversationAgentSwitch>, ConversationError> {
        if !self.holds_switch(id, pending).await? {
            return Ok(None);
        }
        self.finish_switch(id, pending, true).await.map(Some)
    }

    /// Whether the conversation has turns a restart or a failure left owed,
    /// which the next opening resumes. Asked of the durable marks, not of
    /// the copy resumption keeps, which is filled only after start.
    async fn owes_turns(&self, id: &ConversationId) -> Result<bool, ConversationError> {
        let Some(marks) = self.inner.unfinished_turns.get() else {
            return Ok(false);
        };
        Ok(marks
            .marked()
            .await?
            .iter()
            .any(|mark| mark.conversation_id == *id))
    }

    /// Whether the committed history holds `pending`'s handoff.
    async fn holds_switch(
        &self,
        id: &ConversationId,
        pending: &ConversationAgentSwitch,
    ) -> Result<bool, ConversationError> {
        Ok(self
            .committed_if_present_with_order(id)
            .await?
            .and_then(|(committed, _)| {
                committed.snapshot().map(|snapshot| {
                    snapshot
                        .handoff(pending.initiator_principal_id.as_str(), &pending.request_id)
                        .is_some()
                })
            })
            .unwrap_or(false))
    }

    /// Settle `pending` as the committed history decided: applied when it
    /// holds the switch's handoff, not applied when it does not. Asked only
    /// once nothing running can save it any more.
    async fn finish_switch(
        &self,
        id: &ConversationId,
        pending: &ConversationAgentSwitch,
        held: bool,
    ) -> Result<ConversationAgentSwitch, ConversationError> {
        if held {
            self.invalidate_selection_usage_limit(
                SelectionChange::Agent,
                id,
                &pending.request_id,
                ScheduleActor {
                    person: true,
                    organization_id: Some(pending.organization_id.as_str().to_owned()),
                    principal_id: Some(pending.initiator_principal_id.as_str().to_owned()),
                    surface_id: Some(pending.initiator_surface_id.clone()),
                    action_id: Some(pending.request_id.clone()),
                },
            )
            .await?;
        }
        let state = if held {
            ConversationAgentSwitchState::Applied
        } else {
            ConversationAgentSwitchState::NotApplied
        };
        let settled = self
            .inner
            .metadata
            .finish_agent_switch(id, &pending.request_id, state)
            .await?;
        tracing::info!(conversation_id = %id, agent = settled.requested_agent.name(), applied = held, "an agent switch settled");
        self.inner.live_changes.publish(id);
        Ok(settled)
    }

    /// The caller holds the submission lock. Opening can leave a saved switch
    /// pending when its durable selection invalidation failed; no new turn may
    /// become that old request's causal limit during its later recovery.
    pub(super) async fn finish_agent_switch_before_submission(
        &self,
        id: &ConversationId,
    ) -> Result<(), ConversationError> {
        if let Some(pending) = self.inner.metadata.pending_agent_switch(id).await? {
            if self.settle_if_saved(id, &pending).await?.is_none() {
                return Err(ConversationError::RequestConflict);
            }
        }
        Ok(())
    }

    /// What the next opening of `record` runs: a pending switch's agent and
    /// model, with the switch for its history to record; otherwise the
    /// record's own, with the fork this conversation was created as, when
    /// one is waiting for it.
    pub(super) async fn opening_plan(
        &self,
        record: &Conversation,
    ) -> Result<OpeningPlan, ConversationError> {
        if let Some(switch) = self
            .inner
            .metadata
            .pending_agent_switch(record.id())
            .await?
        {
            let actor = ActionContext::new(
                switch.initiator_principal_id.as_str(),
                &switch.initiator_surface_id,
                &switch.request_id,
            )
            .map_err(|_| ConversationError::Metadata)?;
            return Ok(OpeningPlan {
                agent: switch.requested_agent,
                model: switch.requested_model.clone(),
                handoff: Some(HandoffIntent::Switch {
                    actor,
                    at_ms: switch.requested_at_ms,
                }),
                switch: Some(switch),
            });
        }
        // Only the creation that asked for the fork takes its seed: another
        // request's creation of the same id records no copy it never asked for.
        let handoff = {
            let mut seeds = self.inner.forks.lock().expect("fork seeds");
            let asked = seeds.get(record.id()).is_some_and(|seed| {
                seed.request == record.creation_action() && seed.surface == record.creator_surface()
            });
            if asked {
                seeds.remove(record.id()).map(|seed| seed.intent)
            } else {
                None
            }
        };
        Ok(OpeningPlan {
            agent: record.agent().ok_or(ConversationError::AgentUnsupported)?,
            model: record.model().clone(),
            handoff,
            switch: None,
        })
    }

    /// Create `id` as a fork of `source`: a copy of its turns, on its agent,
    /// model, approval preset and environment, listed as it is. Through the
    /// creation command, so a retry of this request answers as the first did.
    pub async fn fork(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        source: ConversationId,
    ) -> Result<CreationReceipt, CreationFailure<ConversationError>> {
        let target = CreationFailure::Target;
        if id == source {
            return Err(target(ConversationError::InvalidInput));
        }
        // A fork's target is a conversation this request creates: a new one,
        // or this request's own after a stop. Any other conversation is
        // refused, never reopened as if it were the fork. This request's own
        // is seeded again only while it has said nothing, so an opening after
        // its fork was saved is never given a second one.
        match load_conversation(self.inner.metadata.as_ref(), &id)
            .await
            .map_err(target)?
        {
            None => {}
            Some(existing)
                if existing.creation_action() != caller.action_id
                    || existing.creator_surface() != caller.surface_id =>
            {
                return Err(CreationFailure::Conflict);
            }
            Some(existing) => {
                let said = self
                    .committed_if_present_with_order(&id)
                    .await
                    .map_err(target)?
                    .and_then(|(committed, _)| {
                        committed.snapshot().map(|snapshot| {
                            !snapshot.invocations.is_empty() || !snapshot.handoffs.is_empty()
                        })
                    })
                    .unwrap_or(false);
                if said {
                    // Its fork is saved: a retry is answered by the creation
                    // it made, asked for as it was then. The source is not
                    // read again, so a source deleted, closed to the caller
                    // or moved to another model since cannot change it.
                    let environment = self
                        .inner
                        .environment
                        .placement(&id)
                        .await
                        .map_err(target)?;
                    let channel = self.channel_id_of(&id).await.map_err(target)?;
                    return self
                        .create_command(
                            storage,
                            id,
                            caller,
                            RequestedConversation {
                                agent: existing.agent().map(RequestedAgent::Known),
                                model: Some(existing.model().as_str().into()),
                                approval_mode: Some(existing.approval_mode()),
                                environment,
                                channel,
                            },
                        )
                        .await;
                }
            }
        }
        let record = load_conversation(self.inner.metadata.as_ref(), &source)
            .await
            .map_err(target)?
            .ok_or(target(ConversationError::NotFound))?;
        self.access()
            .admit(&record, &caller)
            .map_err(|error| target(error.into()))?;
        let agent = record
            .agent()
            .ok_or(target(ConversationError::AgentUnsupported))?;
        let history = self
            .committed_if_present_with_order(&source)
            .await
            .map_err(target)?
            .and_then(|(committed, _)| {
                committed
                    .snapshot()
                    .map(|snapshot| snapshot.carried_history())
            })
            .unwrap_or_default();
        let environment = self
            .inner
            .environment
            .placement(&source)
            .await
            .map_err(target)?;
        // A fork stays in its source's channel, as it stays on its host.
        let channel = self.channel_id_of(&source).await.map_err(target)?;
        let actor = caller.actor().map_err(target)?;
        let at_ms = self.inner.clock.unix_milliseconds();
        let request = caller.action_id.clone();
        // The seed this call places: only it is let go below.
        let seeded = NEXT_SEED.fetch_add(1, Ordering::Relaxed);
        // One fork into an id at a time: another request's seed is never
        // replaced, so an opening records the fork its creation asked for.
        match self
            .inner
            .forks
            .lock()
            .expect("fork seeds")
            .entry(id.clone())
        {
            // This request's own, still waiting, is a retry while the
            // first runs: answered as a request still running is, never
            // a creation of its own that could open the target without
            // the seed once the first has let it go.
            std::collections::hash_map::Entry::Occupied(seed) => {
                let own = seed.get().request == request && seed.get().surface == caller.surface_id;
                return Err(if own {
                    CreationFailure::Storage(CreationStorageError::Storage(StorageError::Busy))
                } else {
                    CreationFailure::Conflict
                });
            }
            std::collections::hash_map::Entry::Vacant(seed) => {
                seed.insert(ForkSeed {
                    token: seeded,
                    request: request.clone(),
                    surface: caller.surface_id.clone(),
                    intent: HandoffIntent::Fork {
                        source: super::conversation_session(&source),
                        history,
                        actor,
                        at_ms,
                    },
                });
            }
        }
        let created = self
            .create_command(
                storage,
                id.clone(),
                caller,
                RequestedConversation {
                    agent: Some(RequestedAgent::Known(agent)),
                    model: Some(record.model().as_str().into()),
                    approval_mode: Some(record.approval_mode()),
                    environment,
                    channel,
                },
            )
            .await;
        // An opening that did not take it, refused or another's, leaves no
        // seed for a later one: a retry of this fork seeds it again.
        // Only this call's own: once an opening took it, a retry may have
        // placed another, which is that retry's to let go.
        {
            let mut seeds = self.inner.forks.lock().expect("fork seeds");
            if seeds.get(&id).is_some_and(|seed| seed.token == seeded) {
                seeds.remove(&id);
            }
        }
        let created = created?;
        self.copy_summary(&source, &id, at_ms).await;
        Ok(created)
    }

    /// List a new fork as its source is listed, unless it has a summary of
    /// its own already: a retry after something was said in it.
    async fn copy_summary(&self, source: &ConversationId, id: &ConversationId, at_ms: u64) {
        let copied = match self.inner.summaries.load(source).await {
            Ok(Some(summary)) => summary,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(conversation_id = %id, %error, "a fork's source summary could not be read; it is listed once something is said");
                return;
            }
        };
        self.change_summary(id, |previous| {
            previous
                .is_none()
                .then(|| ConversationSummary::after_forking(&copied, at_ms))
        })
        .await;
    }
}
