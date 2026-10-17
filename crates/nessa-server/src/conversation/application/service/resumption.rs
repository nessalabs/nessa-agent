//! Turns a restart left unfinished are carried on when the gateway starts
//! (issue #970).
//!
//! ```text
//! submission ──▶ mark the conversation (durable) ──▶ agent takes the turn
//! last receipt settles, agent idle, not retiring ──▶ clear the mark
//! retirement, mode recovery ──▶ Agent::suspend: what was queued or running
//!   is saved as owed
//! start ──▶ resume_unfinished: every marked conversation, a few at a time
//!   ──▶ opened as its owner ──▶ attached ──▶ resume_owed
//! any opening of a marked conversation ──▶ resume_owed
//! resume_owed, under the submission lock:
//!   for each Agent::owed_turns
//!     a share grant's turn whose grant no longer admits it
//!       ──▶ Agent::decline_turn (NotResumed, audited Declined)
//!     ──▶ Agent::resume_turn ──▶ the successor is watched like a submission
//!   nothing left to do ──▶ clear the mark
//! ```
//!
//! The mark only says where to look ([`UnfinishedTurns`]); the conversation's
//! history decides what is owed. A mark left behind costs one opening that
//! finds nothing; a mark missing loses a turn, so it is written before the
//! agent is asked.
use super::{load_conversation, ConversationService, LiveConversation};
use crate::conversation::application::{
    ConversationCaller, ConversationError, RevisionHold, UnfinishedTurns,
};
use futures_util::{stream, StreamExt};
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationId};
use nessa_sdk::application::agent_execution::{
    agents::{AttachmentPhase, OwedTurn, Resumed},
    permissions::ActionContext,
};
use nessa_sdk::domain::agent_execution::executions::Initiator;
use std::sync::Arc;
use uuid::Uuid;

/// What [`ConversationService::resume_decision`] made of one owed turn.
enum Decision {
    /// Resume it, holding this grant revision across the resume, if any.
    Resume(Option<RevisionHold>),
    /// Leave it ended: its grant no longer admits it.
    Decline,
    /// Not decidable now: leave it owed, and the mark set.
    Unknown,
}

/// Whether `live` runs in an approval mode other than Ask, which no share
/// grant's turn is admitted in.
async fn non_default_mode(live: &LiveConversation) -> bool {
    live.projection
        .lock()
        .await
        .view()
        .selection
        .as_ref()
        .is_some_and(|selection| selection.approval_mode != ConversationApprovalMode::Ask)
}

/// How many marked conversations the startup sweep opens at once. Each
/// opening launches its agent, so the sweep is paced rather than unbounded.
const RESUME_CONCURRENCY: usize = 32;

impl ConversationService {
    /// Bind where conversations with unfinished turns are marked. A second
    /// bind is ignored. Unbound, nothing is marked and nothing is resumed.
    pub fn bind_unfinished_turns(&self, marks: Arc<dyn UnfinishedTurns>) {
        let _ = self.inner.unfinished_turns.set(marks);
    }

    /// Open every conversation marked as having unfinished turns, and carry
    /// on what its history says is owed. Run once at start, in the
    /// background. Returns how many were marked.
    ///
    /// A conversation that cannot be opened now (every agent slot taken, its
    /// agent not built) stays marked: its next opening resumes it, and so
    /// does the next start.
    ///
    /// # Errors
    /// The store's error when the marks cannot be read at all.
    pub async fn resume_unfinished(&self) -> Result<usize, ConversationError> {
        let Some(marks) = self.inner.unfinished_turns.get() else {
            return Ok(0);
        };
        let marked = marks.marked().await?;
        self.unfinished_marked().extend(marked.iter().cloned());
        let count = marked.len();
        stream::iter(marked)
            .for_each_concurrent(RESUME_CONCURRENCY, |id| async move {
                if self.inner.retirement.get().is_some() {
                    return;
                }
                if let Err(error) = self.resume_conversation(&id).await {
                    tracing::warn!(
                        conversation_id = %id,
                        %error,
                        "unfinished turns could not be resumed; the next opening tries again"
                    );
                }
            })
            .await;
        Ok(count)
    }

    async fn resume_conversation(&self, id: &ConversationId) -> Result<(), ConversationError> {
        let Some(record) = load_conversation(self.inner.metadata.as_ref(), id).await? else {
            return self.clear_mark(id).await;
        };
        // A deleted conversation has nothing to resume, and its deletion
        // stops whatever it had.
        if record.deletion().is_some() {
            return self.clear_mark(id).await;
        }
        let caller = ConversationCaller {
            organization_id: record.organization().clone(),
            principal_id: record.owner().clone(),
            surface_id: "gateway".into(),
            action_id: format!("resume_after_restart:{}", Uuid::new_v4()),
        };
        let live = self.resolve(id, &caller).await?;
        self.resume_owed(id, live).await;
        Ok(())
    }

    /// Carry on every turn `live`'s history says is owed, when `id` is
    /// marked. Each is resumed by the gateway, under the submission lock, so
    /// it is ordered with every person's message; a share grant's turn only
    /// while that grant still admits it, as a fresh turn of its would be.
    pub(super) async fn resume_owed(&self, id: &ConversationId, live: Arc<LiveConversation>) {
        if self.inner.unfinished_turns.get().is_none() || !self.unfinished_marked().contains(id) {
            return;
        }
        let _ = live.join_attachment_owner().await;
        if live.agent.attachment_status().phase() != AttachmentPhase::Attached {
            return;
        }
        // Not held across the wait for the lock: a delete holds it while it
        // waits for this agent's history to be let go.
        let waiting = Arc::downgrade(&live);
        drop(live);
        let Ok(_admission) = self.admit().await else {
            return;
        };
        let _submissions = self.inner.mode_changes.lock(id).await;
        let Some(live) = waiting.upgrade() else {
            return;
        };
        // Its approval mode is not settled: nothing runs until it is, and
        // the next opening resumes it.
        if !matches!(self.inner.metadata.pending_mode_change(id).await, Ok(None)) {
            return;
        }
        let Ok(actor) = ActionContext::new(
            "gateway",
            "resume_after_restart",
            Uuid::new_v4().to_string(),
        ) else {
            return;
        };
        let non_default_mode = non_default_mode(&live).await;
        let mut resumed = Vec::new();
        let mut unsettled = false;
        for turn in live.agent.owed_turns().await {
            let held = match self
                .resume_decision(&live, id, &turn, non_default_mode)
                .await
            {
                Decision::Resume(held) => held,
                Decision::Decline => {
                    if let Err(error) = live.agent.decline_turn(&turn, actor.clone()).await {
                        unsettled = true;
                        tracing::warn!(conversation_id = %id, execution_id = %turn.execution_id().as_str(), %error, "a declined turn could not be recorded; the next opening tries again");
                    }
                    continue;
                }
                Decision::Unknown => {
                    unsettled = true;
                    continue;
                }
            };
            match live.agent.resume_turn(&turn, actor.clone()).await {
                Ok(Resumed::Continued(admission)) => resumed.push(admission),
                Ok(Resumed::ContinuedUnaudited { admission, error }) => {
                    tracing::error!(conversation_id = %id, execution_id = %turn.execution_id().as_str(), %error, "a resumed turn's audit record could not be saved");
                    resumed.push(admission);
                }
                Ok(Resumed::Exhausted) => tracing::warn!(
                    conversation_id = %id,
                    execution_id = %turn.execution_id().as_str(),
                    "a turn cut off on every start is left ended"
                ),
                Ok(Resumed::NotOwed) => {}
                Err(error) => {
                    unsettled = true;
                    tracing::warn!(
                        conversation_id = %id,
                        execution_id = %turn.execution_id().as_str(),
                        %error,
                        "an unfinished turn could not be resumed; the next opening tries again"
                    );
                }
            }
            drop(held);
        }
        if !unsettled {
            self.clear_if_nothing_owed(id, &live).await;
        }
        for admission in resumed {
            self.watch_receipt(id, live.clone(), admission, true).await;
        }
    }

    /// Whether `turn` is resumed, left ended, or cannot be decided now. The
    /// owner's turn is always resumed. A share grant's only while the grant
    /// still admits it as a fresh turn of its would be, with its revision
    /// held across the resume as a fresh turn holds it across its enqueue: a
    /// revoke lands before (declined here) or after (applies to the
    /// successor's calls). A grant whose policy cannot be read now is not
    /// decided, so a passing storage failure never ends a turn.
    async fn resume_decision(
        &self,
        live: &LiveConversation,
        id: &ConversationId,
        turn: &OwedTurn,
        non_default_mode: bool,
    ) -> Decision {
        let Initiator::Grant(grant) = turn.initiation().initiator() else {
            return Decision::Resume(None);
        };
        match self
            .check_drive_turn(live, id, grant, non_default_mode)
            .await
        {
            Ok(Err(_)) => return Decision::Decline,
            Err(_) => return Decision::Unknown,
            Ok(Ok(())) => {}
        }
        let Some(drive) = self.inner.drive.get() else {
            return Decision::Decline;
        };
        match drive
            .grants
            .hold_revision(id, grant.grant(), grant.policy_revision())
            .await
        {
            Ok(held) => Decision::Resume(Some(held)),
            Err(ConversationError::Revoked | ConversationError::DrivePolicyUnenforceable) => {
                Decision::Decline
            }
            Err(_) => Decision::Unknown,
        }
    }

    /// Clear `id`'s mark when its agent is idle and its history lists no
    /// owed turn: each was resumed, or declined, which ends it in history.
    /// Read-only but for the mark. Called under the conversation's
    /// submission lock.
    async fn clear_if_nothing_owed(&self, id: &ConversationId, live: &LiveConversation) {
        if self.inner.retirement.get().is_some()
            || !live.agent.is_idle().await
            || !live.agent.owed_turns().await.is_empty()
        {
            return;
        }
        if let Err(error) = self.clear_mark(id).await {
            tracing::warn!(conversation_id = %id, %error, "a finished conversation's mark could not be cleared");
        }
    }

    /// Mark `id` before its agent is given a turn. Durable before it
    /// returns; a conversation this run already marked writes nothing.
    pub(super) async fn mark_unfinished(
        &self,
        id: &ConversationId,
    ) -> Result<(), ConversationError> {
        let Some(marks) = self.inner.unfinished_turns.get() else {
            return Ok(());
        };
        if self.unfinished_marked().contains(id) {
            return Ok(());
        }
        marks.mark(id).await?;
        self.unfinished_marked().insert(id.clone());
        Ok(())
    }

    /// Clear `id`'s mark once a turn of its settles and its agent has
    /// nothing left to do. Never while retiring: retirement suspends the
    /// agent, and what it stopped is owed to the next start. A submission,
    /// mode change or deletion holding the conversation's lock is left to
    /// it: the next turn to settle asks again, and a mark left behind costs
    /// only an opening that finds nothing.
    pub(super) async fn clear_when_finished(&self, id: &ConversationId, live: &LiveConversation) {
        if self.inner.unfinished_turns.get().is_none()
            || self.inner.retirement.get().is_some()
            || !self.unfinished_marked().contains(id)
        {
            return;
        }
        let Some(_submissions) = self.inner.mode_changes.try_lock(id) else {
            return;
        };
        // Turns a resume could not settle (its mode change pending, a
        // grant's policy unreadable) are still owed, and keep the mark.
        self.clear_if_nothing_owed(id, live).await;
    }

    /// Clear `id`'s mark once its agent is closed for good, logging a mark
    /// that could not be cleared: the next start opens it, finds nothing
    /// owed, and clears it then.
    pub(super) async fn forget_unfinished(&self, id: &ConversationId) {
        if !self.unfinished_marked().contains(id) {
            return;
        }
        if let Err(error) = self.clear_mark(id).await {
            tracing::warn!(conversation_id = %id, %error, "a closed conversation's mark could not be cleared");
        }
    }

    async fn clear_mark(&self, id: &ConversationId) -> Result<(), ConversationError> {
        let Some(marks) = self.inner.unfinished_turns.get() else {
            return Ok(());
        };
        marks.clear(id).await?;
        self.unfinished_marked().remove(id);
        Ok(())
    }

    fn unfinished_marked(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashSet<ConversationId>> {
        self.inner
            .unfinished_marked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
