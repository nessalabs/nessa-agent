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
//! a turn settles, agent idle, history owes work (a failure a retry can fix)
//!   ──▶ defer the mark: failures + 1, retry due after FailureBackoff
//!   ──▶ an agent that awaits its close is closed (its work stays owed)
//!   ──▶ run_failure_retries, at the due time ──▶ opened ──▶ resume_owed
//! a resume that cannot run now (not opened, its attachment failed)
//!   ──▶ retried after the backoff; an agent stuck that way is closed first
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
use std::time::Duration;
use uuid::Uuid;

/// How long a failure's retry waits: `first` after the first failure in a
/// row, twice as long after each next one, never longer than `most`. A turn
/// gets at most `Resumption::MAX_RESUMES` successors, so a failure that
/// repeats as the turn runs ends. A conversation that cannot be opened or
/// take a turn at all is tried `attempts` times in a row, then left to its
/// next opening or the next start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailureBackoff {
    /// The wait after the first failure in a row.
    pub first: Duration,
    /// The longest wait.
    pub most: Duration,
    /// Retries in a row before a conversation that cannot take a turn is
    /// left to its next opening.
    pub attempts: u32,
}
impl Default for FailureBackoff {
    fn default() -> Self {
        // About an hour and a half of trying before it waits for an opening.
        Self {
            first: Duration::from_secs(5),
            most: Duration::from_secs(300),
            attempts: 24,
        }
    }
}
impl FailureBackoff {
    /// The wait before the `failures`th retry in a row (counting from one).
    pub fn delay(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(31);
        self.first.saturating_mul(1 << doublings).min(self.most)
    }
}

/// What this run knows of one marked conversation's failure retries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Retry {
    /// Failure retries in a row so far.
    failures: u32,
    /// When the next is due, in Unix milliseconds; `None` when none waits.
    at_ms: Option<u64>,
}

/// What [`ConversationService::resume_conversation`] could do.
enum Resuming {
    /// Every owed turn was decided, or there was nothing to decide.
    Decided,
    /// Nothing could be decided now, for the agent this opening holds.
    Blocked(std::sync::Weak<LiveConversation>),
}

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

/// Whether `live`'s agent cannot take a turn until it is closed and opened
/// again: it awaits its close, or its attachment failed.
fn stuck(live: &LiveConversation) -> bool {
    live.agent.awaits_close()
        || matches!(
            live.agent.attachment_status().phase(),
            AttachmentPhase::Failed(_)
        )
}

/// How many marked conversations the startup sweep opens at once. Each
/// opening launches its agent, so the sweep is paced rather than unbounded.
const RESUME_CONCURRENCY: usize = 32;

impl ConversationService {
    /// Bind where conversations with unfinished turns are marked, and how
    /// long a failure's retry waits. A second bind is ignored. Unbound,
    /// nothing is marked and nothing is resumed.
    pub fn bind_unfinished_turns(&self, marks: Arc<dyn UnfinishedTurns>, backoff: FailureBackoff) {
        if self.inner.unfinished_turns.set(marks).is_ok() {
            let _ = self.inner.failure_backoff.set(backoff);
        }
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
        let count = marked.len();
        let now = self.inner.clock.unix_milliseconds();
        let mut due = Vec::with_capacity(count);
        {
            let mut known = self.unfinished_marked();
            for mark in marked {
                // A retry still waiting is the retry loop's to run.
                let waiting = mark.retry_at_ms.filter(|at| *at > now);
                known.insert(
                    mark.conversation_id.clone(),
                    Retry {
                        failures: mark.failures,
                        at_ms: waiting,
                    },
                );
                if waiting.is_none() {
                    due.push(mark.conversation_id);
                }
            }
        }
        self.inner.retry_scheduled.notify_one();
        stream::iter(due)
            .for_each_concurrent(RESUME_CONCURRENCY, |id| async move {
                if self.inner.retirement.get().is_some() {
                    return;
                }
                self.resume_or_retry(&id).await;
            })
            .await;
        Ok(count)
    }

    /// Open `id` and resume what it owes. When that cannot be done now (it
    /// cannot be opened, or its agent cannot take a turn: the provider is
    /// still down), its retry is scheduled after a backoff, and an agent
    /// that cannot recover in place is closed so the retry opens a new one.
    async fn resume_or_retry(&self, id: &ConversationId) {
        let blocked = match self.resume_conversation(id).await {
            Ok(Resuming::Decided) => return,
            Ok(Resuming::Blocked(live)) => Some(live),
            Err(error) => {
                tracing::warn!(conversation_id = %id, %error, "unfinished turns could not be resumed; retrying after a backoff");
                None
            }
        };
        let spent = self.inner.failure_backoff.get().is_some_and(|backoff| {
            self.unfinished_marked()
                .get(id)
                .is_some_and(|retry| retry.failures >= backoff.attempts)
        });
        if spent {
            tracing::warn!(conversation_id = %id, "a conversation that cannot take its retry is left to its next opening");
        }
        if spent
            || self.inner.retirement.get().is_some()
            || !self.unfinished_marked().contains_key(id)
        {
            return;
        }
        let mut failed = true;
        if let Some(waiting) = blocked {
            // Under the submission lock, as `after_settle` releases, so a
            // person's message that closes the same agent is ordered with it.
            let _submissions = self.inner.mode_changes.lock(id).await;
            if let Some(live) = waiting.upgrade() {
                if stuck(&live) {
                    // Released before the retry is due, so the retry never
                    // meets the closing agent.
                    if live.agent.is_idle().await {
                        self.release_for_reopen(id, live).await;
                    }
                } else {
                    // Still connecting is not a failure: it waits as long
                    // again, and does not count toward `attempts`.
                    failed = !matches!(
                        live.agent.attachment_status().phase(),
                        AttachmentPhase::Waiting | AttachmentPhase::Starting
                    );
                }
            }
        }
        self.schedule_retry(id, failed).await;
    }

    async fn resume_conversation(
        &self,
        id: &ConversationId,
    ) -> Result<Resuming, ConversationError> {
        let Some(record) = load_conversation(self.inner.metadata.as_ref(), id).await? else {
            return self.clear_mark(id).await.map(|()| Resuming::Decided);
        };
        // A deleted conversation has nothing to resume, and its deletion
        // stops whatever it had.
        if record.deletion().is_some() {
            return self.clear_mark(id).await.map(|()| Resuming::Decided);
        }
        let caller = ConversationCaller {
            organization_id: record.organization().clone(),
            principal_id: record.owner().clone(),
            surface_id: "gateway".into(),
            action_id: format!("resume_owed:{}", Uuid::new_v4()),
        };
        let live = self.resolve(id, &caller).await?;
        let opened = Arc::downgrade(&live);
        Ok(if self.resume_owed(id, live).await {
            Resuming::Decided
        } else {
            Resuming::Blocked(opened)
        })
    }

    /// Carry on every turn `live`'s history says is owed, when `id` is
    /// marked. Each is resumed by the gateway, under the submission lock, so
    /// it is ordered with every person's message; a share grant's turn only
    /// while that grant still admits it, as a fresh turn of its would be.
    /// False when nothing could be decided now: `id` is not marked, its
    /// agent cannot take a turn, it is retiring, or its approval mode is
    /// not settled.
    pub(super) async fn resume_owed(
        &self,
        id: &ConversationId,
        live: Arc<LiveConversation>,
    ) -> bool {
        if self.inner.unfinished_turns.get().is_none() || !self.unfinished_marked().contains_key(id)
        {
            return false;
        }
        let _ = live.join_attachment_owner().await;
        // An agent that is stuck takes no turn: its retry closes it and
        // opens a new one (`resume_or_retry`, `after_settle`).
        if stuck(&live)
            || !matches!(
                live.agent.attachment_status().phase(),
                AttachmentPhase::Attached | AttachmentPhase::Absent
            )
        {
            return false;
        }
        // Not held across the wait for the lock: a delete holds it while it
        // waits for this agent's history to be let go.
        let waiting = Arc::downgrade(&live);
        drop(live);
        let Ok(_admission) = self.admit().await else {
            return false;
        };
        let _threads = self.inner.thread_admission.clone().lock_owned().await;
        let _submissions = self.inner.mode_changes.lock(id).await;
        let Some(live) = waiting.upgrade() else {
            return false;
        };
        // Its approval mode is not settled: nothing runs until it is, and
        // the next opening resumes it.
        if !matches!(self.inner.metadata.pending_mode_change(id).await, Ok(None)) {
            return false;
        }
        let Ok(actor) = ActionContext::new("gateway", "resume_owed", Uuid::new_v4().to_string())
        else {
            return false;
        };
        let non_default_mode = non_default_mode(&live).await;
        let mut resumed = Vec::new();
        let mut unsettled = false;
        let Ok(Some(owner)) = load_conversation(self.inner.metadata.as_ref(), id).await else {
            return false;
        };
        for turn in live.agent.owed_turns().await {
            let stop_caller = ConversationCaller {
                organization_id: live.organization_id.clone(),
                principal_id: owner.owner().clone(),
                surface_id: turn.submitted_by().surface_id().into(),
                action_id: turn.submitted_by().request_id().into(),
            };
            match self
                .captured_stop_actor(id, turn.execution_id().as_str(), &stop_caller)
                .await
            {
                Ok(Some(stopper)) => {
                    if let Err(error) = live.agent.decline_turn(&turn, stopper).await {
                        tracing::warn!(conversation_id = %id, %error, "captured Stop decline failed");
                        unsettled = true;
                    }
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(conversation_id = %id, %error, "captured Stop selection cannot be checked");
                    unsettled = true;
                    continue;
                }
            }
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
        drop(_submissions);
        drop(_threads);
        for admission in resumed {
            self.watch_receipt(id, live.clone(), admission, true).await;
        }
        // A turn left undecided is retried after a backoff.
        !unsettled
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
        if self.unfinished_marked().contains_key(id) {
            return Ok(());
        }
        marks.mark(id).await?;
        self.unfinished_marked()
            .insert(id.clone(), Retry::default());
        Ok(())
    }

    /// After one of `id`'s turns settles: once its agent is idle, clear the
    /// mark if its history owes nothing, or schedule a retry when a failure
    /// left work owed. Never while retiring: retirement suspends the agent,
    /// and what it stopped is owed to the next start. Under the
    /// conversation's submission lock, so a submission that settled at once
    /// is decided after it returns, and an agent another holder of the lock
    /// closed or replaced meanwhile is left to it.
    pub(super) async fn after_settle(&self, id: &ConversationId, live: Arc<LiveConversation>) {
        if self.inner.unfinished_turns.get().is_none()
            || self.inner.retirement.get().is_some()
            || !self.unfinished_marked().contains_key(id)
        {
            return;
        }
        // Not held across the wait for the lock: a delete holds it while it
        // waits for this agent's history to be let go.
        let waiting = Arc::downgrade(&live);
        drop(live);
        let _submissions = self.inner.mode_changes.lock(id).await;
        let Some(live) = waiting.upgrade() else {
            return;
        };
        if self.inner.retirement.get().is_some() || !live.agent.is_idle().await {
            return;
        }
        // Anything still owed is retried after a backoff: a failure a retry
        // can fix, and a turn a resume could not decide (a grant's policy
        // unreadable).
        if live.agent.owed_turns().await.is_empty() {
            self.clear_if_nothing_owed(id, &live).await;
            return;
        }
        // Released before the retry is due, so the retry never meets the
        // closing agent.
        if stuck(&live) {
            self.release_for_reopen(id, live).await;
        }
        self.schedule_retry(id, true).await;
    }

    /// Close `live`, an agent that cannot take its retry, and let its slot go
    /// so the retry opens the conversation again; what it owed stays owed.
    /// The agent holds its history's lease while any handle to it is kept,
    /// so the retry's opening needs it closed first. Nothing is done when
    /// another opening holds the slot now, or it is already stopping.
    async fn release_for_reopen(&self, id: &ConversationId, live: Arc<LiveConversation>) {
        let slot = self.inner.conversations.lock().await.get(id).cloned();
        let ours = slot.as_ref().is_some_and(
            |slot| matches!(slot.value.get(), Some(Ok(open)) if Arc::ptr_eq(open, &live)),
        );
        drop(live);
        if let (true, Some(slot)) = (ours, slot) {
            if !slot.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                self.reopen_ended(id, slot).await;
            }
        }
    }

    /// Record that `id`'s next failure retry is due after its backoff, and
    /// wake the retry loop; `failed` counts it as one more failure in a row.
    /// A retry already waiting is kept as it is, so a
    /// later settle neither puts it off nor counts as another failure. Nothing
    /// is done when `id` is not marked. A deferral that cannot be saved still
    /// runs in this process; after a restart the mark alone resumes it.
    async fn schedule_retry(&self, id: &ConversationId, failed: bool) {
        let (Some(marks), Some(backoff)) = (
            self.inner.unfinished_turns.get(),
            self.inner.failure_backoff.get(),
        ) else {
            return;
        };
        let failures = match self.unfinished_marked().get(id) {
            None | Some(Retry { at_ms: Some(_), .. }) => return,
            Some(retry) => retry.failures.saturating_add(u32::from(failed)),
        };
        let delay = u64::try_from(backoff.delay(failures.max(1)).as_millis()).unwrap_or(u64::MAX);
        let at_ms = self.inner.clock.unix_milliseconds().saturating_add(delay);
        if let Err(error) = marks.defer(id, failures, at_ms).await {
            tracing::warn!(conversation_id = %id, %error, "a failed turn's retry could not be saved; it still runs, and a restart resumes it");
        }
        tracing::info!(conversation_id = %id, failures, delay_ms = delay, "work is still owed; retrying after a backoff");
        // A clear that raced the deferral wins: the conversation owes nothing.
        if let Some(retry) = self.unfinished_marked().get_mut(id) {
            *retry = Retry {
                failures,
                at_ms: Some(at_ms),
            };
        }
        self.inner.retry_scheduled.notify_one();
    }

    /// Run each failure retry when it is due, until the gateway retires:
    /// the conversation is opened, as its owner, and what its history owes
    /// is resumed. Started once, beside [`Self::resume_unfinished`]. Due
    /// retries are taken a few at a time, as the startup sweep takes marks.
    pub async fn run_failure_retries(&self) {
        loop {
            let scheduled = self.inner.retry_scheduled.notified();
            tokio::pin!(scheduled);
            scheduled.as_mut().enable();
            if self.inner.retirement.get().is_some() {
                return;
            }
            let now = self.inner.clock.unix_milliseconds();
            let mut due = Vec::new();
            let mut next = None::<u64>;
            for (id, retry) in self.unfinished_marked().iter_mut() {
                match retry.at_ms {
                    Some(at) if at <= now => {
                        retry.at_ms = None;
                        due.push(id.clone());
                    }
                    Some(at) => next = Some(next.map_or(at, |next| next.min(at))),
                    None => {}
                }
            }
            stream::iter(due)
                .for_each_concurrent(RESUME_CONCURRENCY, |id| async move {
                    self.resume_or_retry(&id).await;
                })
                .await;
            let wait = next.map(|at| Duration::from_millis(at.saturating_sub(now)));
            tokio::select! {
                () = &mut scheduled => {}
                () = async {
                    match wait {
                        Some(wait) => tokio::time::sleep(wait).await,
                        None => std::future::pending().await,
                    }
                } => {}
                () = self.retired() => return,
            }
        }
    }

    /// Clear `id`'s mark once its agent is closed for good, logging a mark
    /// that could not be cleared: the next start opens it, finds nothing
    /// owed, and clears it then.
    pub(super) async fn forget_unfinished(&self, id: &ConversationId) {
        if !self.unfinished_marked().contains_key(id) {
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
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<ConversationId, Retry>> {
        self.inner
            .unfinished_marked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
