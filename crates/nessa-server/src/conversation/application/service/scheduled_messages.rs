//! One scheduled continuation after a provider usage limit.
//!
//! The preference is one boolean for this gateway, default off. A row is the
//! durable one-shot message. Delivery calls [`ConversationService::submit_command`]
//! with the causal turn's initiation, so Cedar and a deleted conversation are
//! decided there. The compare-and-set from `delivering` to `submitting` is the
//! only permission to submit, and it takes a lease the other processes on this
//! file can see. A retryable submit failure keeps the row `submitting` until
//! a bounded retry time. A settlement whose plan does not commit keeps the
//! same kind of debt and is planned again at that time. Cancel, defer, and a
//! model change lose to that transition and win against a claim that has not
//! made it. An applied model change is not complete until that plan commits.
//! Cedar's allow and deny of the preference are each one audit before the
//! command result. That audit and the request's receipt commit together.
//! A retry returns the receipt and does not apply the change again.

/// How long a `submitting` lease lasts. The owner renews it while the submit runs.
pub(crate) const FENCE_LEASE_MS: u64 = 30_000;

/// How often the owner extends a lease it still holds.
pub(crate) const FENCE_RENEW_EVERY_MS: u64 = 10_000;

/// How long a delivery pass waits after a schedule-store read fails.
pub(crate) const SCHEDULE_RETRY_MS: u64 = 1_000;

/// The longest wait before a failed catch-up scan is tried again.
pub(crate) const CATCH_UP_RETRY_MAX_MS: u64 = 60_000;

/// How long renewal waits after `renew_fence` returns an error.
pub(crate) const FENCE_RENEW_RETRY_MS: u64 = 1_000;

use super::{ConversationCaller, ConversationError, ConversationId, ConversationService};
use crate::conversation::application::usage_limit_policy::{
    UsageLimitPreferenceDecision, UsageLimitPreferenceFacts, UsageLimitPreferencePolicyEvaluator,
};
use crate::conversation::application::ConversationFuture;
use crate::conversation::domain::{ContinuationCause, ContinuationPlan};
use nessa_auth::application::ports::Clock;
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, GrantInitiator, Initiation, Initiator, ToolChecking,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Who an audit row names. A provider limit is the system. The person is named
/// only for the preference change and the cancellations that change causes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleActor {
    pub person: bool,
    pub organization_id: Option<String>,
    pub principal_id: Option<String>,
    pub surface_id: Option<String>,
    pub action_id: Option<String>,
}

impl ScheduleActor {
    /// A provider limit. No person is invented for it.
    pub fn system() -> Self {
        Self {
            person: false,
            organization_id: None,
            principal_id: None,
            surface_id: None,
            action_id: None,
        }
    }

    /// The person who changed the preference.
    pub fn person(caller: &ConversationCaller) -> Self {
        Self {
            person: true,
            organization_id: Some(caller.organization_id.as_str().to_owned()),
            principal_id: Some(caller.principal_id.as_str().to_owned()),
            surface_id: Some(caller.surface_id.clone()),
            action_id: Some(caller.action_id.clone()),
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        if self.person {
            "person"
        } else {
            "system"
        }
    }
}

/// A continuation the store has accepted or is delivering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledMessage {
    pub conversation: String,
    pub causal_execution: String,
    pub execution_id: String,
    pub due_at_ms: u64,
    pub delivering: bool,
    pub agent: String,
    pub model: String,
    pub prompt: String,
    pub organization_id: String,
    pub principal_id: String,
    pub surface_id: String,
    pub action_id: String,
    pub initiation: String,
    pub grant_id: Option<String>,
    pub policy_revision: Option<u64>,
    pub tools_checked: bool,
    /// How many retryable submit attempts have already failed. Zero is a
    /// first attempt. The next defer waits the bounded backoff of this count
    /// plus one, and the row stays `submitting` until that time.
    pub retry_failures: u32,
}

/// A settlement whose continuation plan did not commit, and when to try it again.
///
/// Catch-up being complete does not drop this debt. The delivery loop retries
/// the plan at `retry_at_ms`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileRetry {
    pub conversation: String,
    pub failures: u32,
    pub retry_at_ms: u64,
}

/// The committed history a plan read, and the fence generation it observed.
///
/// `generation` is the `history_fence.generation` at that read, or 0 when the
/// fence had no row. `position` is ordered only inside one `incarnation`. A
/// different incarnation, or a later position, replaces the fence only when
/// `generation` is still the one stored. The same incarnation and position
/// are that sealed history. An older generation writes nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryGeneration {
    pub incarnation: String,
    pub position: u64,
    /// The fence generation this plan observed. 0 is an empty fence.
    pub generation: u64,
}

/// What accepting a row needs. `execution_id` is used only when no row exists
/// yet for this causal turn; a later update keeps the id the row was minted with.
#[derive(Clone, Debug)]
pub struct ScheduleDraft {
    pub conversation: String,
    pub causal_execution: String,
    pub execution_id: String,
    pub due_at_ms: u64,
    pub cause: ContinuationCause,
    pub actor: ScheduleActor,
    pub agent: String,
    pub model: String,
    pub prompt: String,
    pub organization_id: String,
    pub principal_id: String,
    pub surface_id: String,
    pub action_id: String,
    pub initiation: String,
    pub grant_id: Option<String>,
    pub policy_revision: Option<u64>,
    pub tools_checked: bool,
    /// The preference revision the planner read. The insert matches only while
    /// that revision is still enabled.
    pub preference_revision: u64,
    /// The history generation the planner read. The insert matches only while
    /// a newer generation has not been sealed.
    pub history: HistoryGeneration,
}

/// How a fenced submit ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleSettlement {
    /// The execution was saved or the submit admitted it.
    Delivered,
    /// The submit will not succeed on retry.
    Refused,
    /// The journal recorded an attempt and the invocation was not saved.
    Unconfirmed,
}

impl ScheduleSettlement {
    pub(crate) fn status(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Refused => "refused",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

/// One read of the gateway preference: the boolean and the revision that read saw.
///
/// `accept` inserts only while this revision is still enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreferenceReading {
    pub enabled: bool,
    pub revision: u64,
}

/// The preference and the open continuation from one store read.
///
/// `open` is absent when `enabled` is false, including when a row is still
/// stored. The view publishes both from this value, so they cannot disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageLimitSnapshot {
    pub enabled: bool,
    pub open: Option<ScheduledMessage>,
}

/// The preference write, or Cedar's refusal. A refusal leaves the store unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreferenceUpdate {
    /// The stored value after the operation.
    Stored(bool),
    /// Cedar refused. `policies` are the ones that decided.
    Refused { policies: Vec<String> },
}

/// Cedar's decision as it committed, and which enable this request installed.
///
/// `installed_enable` is true only when this transaction changed the boolean
/// from off to on. That write leaves `revision` unsettled. A later scan settles
/// it, and only a still-unsettled revision may be reverted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedPreference {
    pub update: PreferenceUpdate,
    /// The preference revision after this decision committed.
    pub revision: u64,
    /// This decision moved the preference from off to on and left it unsettled.
    pub installed_enable: bool,
    /// This request had already committed. `update` is that result. Nothing
    /// was written, and the caller does not scan again.
    pub replayed: bool,
}

/// What one preference request committed. A retry with the same input returns
/// it. A different input under the same request is a conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreferenceCommandReceipt {
    pub organization_id: String,
    pub principal_id: String,
    pub surface_id: String,
    /// The enabled value the request asked for.
    pub enabled: bool,
    pub outcome: PreferenceCommandOutcome,
}

/// The result stored with a preference request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreferenceCommandOutcome {
    /// The decision committed this value.
    Stored(bool),
    /// Cedar refused. The preference was not changed.
    Refused { policies: Vec<String> },
    /// This request's enable was reverted, in the same transaction as that
    /// rollback's audit. A retry is the same error and does not enable again.
    Failed,
}

/// The private dataset of scheduled continuations.
pub trait ScheduledMessageStore: Send + Sync {
    /// The gateway preference and the revision of that read. Absent means off
    /// at revision 0.
    fn preference(&self) -> ConversationFuture<'_, PreferenceReading>;
    /// Record the preference. Turning it off cancels every open row in the
    /// same transaction. An unchanged value writes nothing.
    fn set_preference(
        &self,
        enabled: bool,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool>;
    /// The open row for one conversation, if it is scheduled or delivering.
    fn open(&self, conversation: &str) -> ConversationFuture<'_, Option<ScheduledMessage>>;
    /// Insert the row, or move its due time when it is still open, only while
    /// `draft.preference_revision` is still the enabled preference and
    /// `draft.history` is still the sealed generation. A finished causal turn,
    /// a revision a preference change has replaced, or a newer history returns
    /// `None`.
    fn accept(
        &self,
        draft: ScheduleDraft,
        at_ms: u64,
    ) -> ConversationFuture<'_, Option<ScheduledMessage>>;
    /// Move a scheduled row's due time and audit that move with the status change.
    fn move_due(
        &self,
        conversation: &str,
        causal_execution: &str,
        due_at_ms: u64,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool>;
    /// Release a delivering claim back to scheduled at a later due time.
    fn defer_claim(
        &self,
        conversation: &str,
        causal_execution: &str,
        due_at_ms: u64,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool>;
    /// Cancel the open row for one conversation.
    fn cancel_open(
        &self,
        conversation: &str,
        cause: ContinuationCause,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool>;
    /// Claim due scheduled rows, then return those claims, rows already
    /// delivering, and `submitting` rows this process may attempt. A
    /// submitting row waiting for a later retry is not claimed. A submitting
    /// row whose lease is still live and owned by another process is not
    /// claimed; [`Self::next_due`] waits until that deadline. Each execution
    /// id appears once. The claim is not itself an audit row: the delivery or
    /// refusal audit follows admission, and a released claim is audited as
    /// `reset_moved`.
    fn claim_due(&self, now_ms: u64, limit: usize)
        -> ConversationFuture<'_, Vec<ScheduledMessage>>;
    /// When the next pass should run. `Some(now_ms)` when a scheduled row is
    /// already due, a row is `delivering`, or a `submitting` row this process
    /// may attempt: its retry time has arrived and its lease has expired or
    /// belongs to this process. A submitting row whose retry time is still
    /// ahead waits until that time. A live lease owned by another process
    /// waits until `fence_until_ms`, or until `retry_at_ms` when that is
    /// later. Otherwise the soonest of a later scheduled due time and a later
    /// submit retry.
    fn next_due(&self, now_ms: u64) -> ConversationFuture<'_, Option<u64>>;
    /// Move `delivering` to `submitting` under a lease that lasts
    /// [`FENCE_LEASE_MS`] from `now_ms`, or adopt a `submitting` row whose
    /// lease has expired or that this process already owns and is not inside,
    /// and whose retry time has arrived. True is the only permission to
    /// submit. The in-memory arm is taken only after the update commits, and
    /// only so this process does not enter twice. Cancel, defer, and a model
    /// change match `scheduled` and `delivering` only, so exactly one of
    /// those and this transition can win.
    fn begin_submit(&self, execution_id: &str, now_ms: u64) -> ConversationFuture<'_, bool>;
    /// Hold a `submitting` row this process owns until `retry_at_ms`.
    /// `failures` is the count after this attempt and must be one more than
    /// the count stored on the row. The status stays `submitting`. The audit
    /// cause is `submission_retry`, written in the same transaction as the
    /// timestamp. `Ok(false)` is a row this process does not own at that
    /// count. `Err` rolls the timestamp and the audit back.
    fn defer_submit(
        &self,
        execution_id: &str,
        failures: u32,
        retry_at_ms: u64,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool>;
    /// Extend this process's lease on a `submitting` row, only while that
    /// lease has not expired. False when this process is not the owner, the
    /// row is no longer `submitting`, or `fence_until_ms` is at or before
    /// `now_ms`. An expired lease is not extended.
    fn renew_fence(&self, execution_id: &str, now_ms: u64) -> ConversationFuture<'_, bool>;
    /// Drop this process's in-memory hold on a `submitting` row without
    /// changing its status or its lease. This process can enter again. Another
    /// process waits until the lease expires.
    fn release_submit(&self, execution_id: &str) -> ConversationFuture<'_, ()>;
    /// When the preference is off, cancel `submitting` rows whose lease has
    /// expired. A live lease is left to finish its submit.
    fn retire_unfenced_submissions(&self, at_ms: u64) -> ConversationFuture<'_, usize>;
    /// The earliest still-live `submitting` lease, so the off path can wake
    /// and cancel it when it expires.
    fn next_live_fence(&self, now_ms: u64) -> ConversationFuture<'_, Option<u64>>;
    /// The fence generation stored for `conversation`, or 0 when none is sealed.
    /// Read this before the session, then put it on [`HistoryGeneration`].
    /// The default is an empty fence.
    fn history_generation(&self, conversation: &str) -> ConversationFuture<'_, u64> {
        let _ = conversation;
        Box::pin(async { Ok(0) })
    }
    /// Seal `history` as the generation this plan observed. A later position,
    /// or a different incarnation, replaces the fence only when `history`
    /// observed the generation now stored. An older generation writes nothing
    /// and returns false. The same incarnation and position are already
    /// sealed and return true. When `dismiss_open` is true and this
    /// generation is current, an open `scheduled` or `delivering` row is
    /// cancelled in that same transaction. The default seals nothing.
    fn note_history(
        &self,
        conversation: &str,
        history: &HistoryGeneration,
        dismiss_open: bool,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool> {
        let _ = (conversation, history, dismiss_open, actor, at_ms);
        Box::pin(async { Ok(true) })
    }
    /// Mark this request's receipt failed. The preference and any other
    /// request stay as they are. The default records nothing.
    fn fail_preference_command(&self, request_id: &str) -> ConversationFuture<'_, ()> {
        let _ = request_id;
        Box::pin(async { Ok(()) })
    }
    /// The receipt for `request_id`, when this store has committed one.
    /// The default is none, for a double that does not record commands.
    fn preference_command(
        &self,
        request_id: &str,
    ) -> ConversationFuture<'_, Option<PreferenceCommandReceipt>> {
        let _ = request_id;
        Box::pin(async { Ok(None) })
    }
    /// The preference and the open row from one read. A durable store reads
    /// both in one transaction. Off reports no open row.
    fn usage_limit_snapshot(
        &self,
        conversation: &str,
    ) -> ConversationFuture<'_, UsageLimitSnapshot> {
        let conversation = conversation.to_owned();
        Box::pin(async move {
            let reading = self.preference().await?;
            if !reading.enabled {
                return Ok(UsageLimitSnapshot {
                    enabled: false,
                    open: None,
                });
            }
            Ok(UsageLimitSnapshot {
                enabled: true,
                open: self.open(&conversation).await?,
            })
        })
    }
    /// Record Cedar's decision. Allow writes the preference, the audit, the
    /// request receipt, and any cancellations in one transaction. Deny writes
    /// the audit and the receipt. `Err` means that transaction did not commit.
    /// An unchanged allow is still audited. An allow that changes off to on
    /// installs one unsettled revision; any other allow leaves the revision
    /// where it was, or settles a disable. A request that already has a
    /// receipt returns it and writes nothing. A different input for that
    /// request is a conflict.
    fn record_preference_decision(
        &self,
        allowed: bool,
        enabled: bool,
        policies: &[String],
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, RecordedPreference>;
    /// Mark `revision` settled when it is still the enabled preference.
    /// True when this call settled it, or it was already settled at that
    /// same revision. False when the preference was reverted or replaced.
    fn settle_installed_preference(&self, revision: u64) -> ConversationFuture<'_, bool>;
    /// Turn off the unsettled enable at `revision`, as `actor`, and cancel
    /// open rows in that transaction. False when that revision is no longer
    /// an unsettled enable, which leaves the preference as it stands.
    fn revert_installed_preference(
        &self,
        revision: u64,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool>;
    /// The row's status text, when it exists.
    fn status(&self, execution_id: &str) -> ConversationFuture<'_, Option<String>>;
    /// After the outcome has been observed. The status change and the audit
    /// commit together. A `submitting` row changes only when `fence_owner` is
    /// this process; a former owner writes no status and no audit. A
    /// `delivering` row has no owner yet. `Err` leaves the previous status.
    /// `at_ms` is the clock at the observation, not a reading taken before
    /// the submit.
    fn record_outcome(
        &self,
        execution_id: &str,
        outcome: ScheduleSettlement,
        cause: &str,
        at_ms: u64,
    ) -> ConversationFuture<'_, ()>;
    /// Blank the prompt and identifying fields of every row for `conversation`.
    /// `scheduled`, `delivering`, and a `submitting` row whose lease has
    /// expired become `cancelled`. A live lease keeps its status. Terminal
    /// rows keep theirs. One new audit per row that still held a payload. A
    /// second call writes nothing. Older audit rows stay.
    fn erase_conversation(&self, conversation: &str, at_ms: u64) -> ConversationFuture<'_, ()>;
    /// Remember that planning `conversation` failed. The next attempt is
    /// `retry_at_ms`. `failures` counts attempts so far, including this one.
    /// Replaces any earlier debt for that conversation.
    fn note_reconcile_retry(
        &self,
        conversation: &str,
        failures: u32,
        retry_at_ms: u64,
    ) -> ConversationFuture<'_, ()>;
    /// Drop the reconcile debt after a plan commits.
    fn clear_reconcile_retry(&self, conversation: &str) -> ConversationFuture<'_, ()>;
    /// Every reconcile debt, due and still ahead.
    fn reconcile_retries(&self) -> ConversationFuture<'_, Vec<ReconcileRetry>>;
}

/// The result a preference request already committed, or a conflict when this
/// call is not that request.
fn replay_preference_command(
    saved: &PreferenceCommandReceipt,
    enabled: bool,
    caller: &ConversationCaller,
) -> Result<PreferenceUpdate, ConversationError> {
    if saved.organization_id != caller.organization_id.as_str()
        || saved.principal_id != caller.principal_id.as_str()
        || saved.surface_id != caller.surface_id
        || saved.enabled != enabled
    {
        return Err(ConversationError::RequestConflict);
    }
    match &saved.outcome {
        PreferenceCommandOutcome::Stored(value) => Ok(PreferenceUpdate::Stored(*value)),
        PreferenceCommandOutcome::Refused { policies } => Ok(PreferenceUpdate::Refused {
            policies: policies.clone(),
        }),
        PreferenceCommandOutcome::Failed => Err(ConversationError::Unavailable),
    }
}

impl ConversationService {
    /// Bind the continuation store. A second bind is ignored.
    pub fn bind_scheduled_messages(&self, store: Arc<dyn ScheduledMessageStore>) {
        let _ = self.inner.scheduled_messages.set(store);
    }

    /// Bind the preference policy. A second bind is ignored. Unbound refuses
    /// the change.
    pub fn bind_usage_limit_preference_policy(
        &self,
        policy: Arc<dyn UsageLimitPreferencePolicyEvaluator>,
    ) {
        let _ = self.inner.preference_policy.set(policy);
    }

    fn scheduled_messages(&self) -> Option<Arc<dyn ScheduledMessageStore>> {
        self.inner.scheduled_messages.get().cloned()
    }

    /// The gateway preference. Unbound is off.
    pub async fn usage_limit_auto_continue(&self) -> Result<bool, ConversationError> {
        match self.scheduled_messages() {
            Some(store) => Ok(store.preference().await?.enabled),
            None => Ok(false),
        }
    }

    /// The open continuation for `conversation`, when the preference is on.
    pub async fn usage_limit_schedule(
        &self,
        conversation: &ConversationId,
    ) -> Result<Option<ScheduledMessage>, ConversationError> {
        Ok(self.usage_limit_snapshot(conversation).await?.open)
    }

    /// The preference and the open continuation from one store read.
    /// Unbound storage is off and has no row.
    pub async fn usage_limit_snapshot(
        &self,
        conversation: &ConversationId,
    ) -> Result<UsageLimitSnapshot, ConversationError> {
        let Some(store) = self.scheduled_messages() else {
            return Ok(UsageLimitSnapshot {
                enabled: false,
                open: None,
            });
        };
        store.usage_limit_snapshot(&conversation.to_string()).await
    }

    /// Turn the preference on or off. The caller's action is checked before
    /// Cedar or any write. A request that already committed returns that
    /// result and does not apply again. Otherwise Cedar decides. Off cancels
    /// open rows as this person. On scans every catalogue owner and schedules
    /// each currently blocked eligible turn. A failed scan reverts only the
    /// unsettled revision this call installed, and that revert marks this
    /// request failed in the same transaction. A finished scan settles the
    /// revision it observed.
    pub async fn set_usage_limit_auto_continue(
        &self,
        enabled: bool,
        caller: ConversationCaller,
        facts: UsageLimitPreferenceFacts,
    ) -> Result<PreferenceUpdate, ConversationError> {
        caller.actor()?;
        if caller.organization_id.as_str() != facts.organization_id
            || caller.principal_id.as_str() != facts.principal_id
        {
            return Err(ConversationError::Unavailable);
        }
        let Some(store) = self.scheduled_messages() else {
            return Err(ConversationError::Unavailable);
        };
        let Some(policy) = self.inner.preference_policy.get().cloned() else {
            return Err(ConversationError::Unavailable);
        };
        if let Some(saved) = store.preference_command(&caller.action_id).await? {
            return replay_preference_command(&saved, enabled, &caller);
        }
        let decision = policy
            .evaluate(&facts)
            .map_err(|_| ConversationError::Unavailable)?;
        let (allowed, policies) = match decision {
            UsageLimitPreferenceDecision::Allow { policies } => (true, policies),
            UsageLimitPreferenceDecision::Deny { policies } => (false, policies),
        };
        let at_ms = self.inner.clock.unix_milliseconds();
        let recorded = store
            .record_preference_decision(
                allowed,
                enabled,
                &policies,
                ScheduleActor::person(&caller),
                at_ms,
            )
            .await?;
        let PreferenceUpdate::Stored(stored) = recorded.update else {
            return Ok(recorded.update);
        };
        if recorded.replayed {
            return Ok(PreferenceUpdate::Stored(stored));
        }
        if stored {
            let mut visited = Vec::new();
            if let Err(error) = self
                .schedule_every_owner(&store, true, ScheduleActor::person(&caller), &mut visited)
                .await
            {
                if recorded.installed_enable {
                    let now = self.inner.clock.unix_milliseconds();
                    let reverted = store
                        .revert_installed_preference(
                            recorded.revision,
                            ScheduleActor::system(),
                            now,
                        )
                        .await?;
                    // The revert marks the request that installed this
                    // revision. A revert that no longer matches leaves that
                    // receipt stored, so this request is marked failed here.
                    if !reverted {
                        store.fail_preference_command(&caller.action_id).await?;
                    }
                } else {
                    // This request did not install the preference. The scan
                    // still failed, so its receipt is failed and a retry does
                    // not report success. Another request's revision stays.
                    store.fail_preference_command(&caller.action_id).await?;
                }
                self.publish_preference(&visited);
                return Err(error);
            }
            // A scan that finished settles the revision it saw, including one
            // a concurrent request installed. That request's later revert then
            // matches nothing.
            if !store.settle_installed_preference(recorded.revision).await? {
                // A newer revision won. This receipt must not stay stored,
                // or a retry reports the enable that revision replaced.
                store.fail_preference_command(&caller.action_id).await?;
                self.publish_preference(&visited);
                return Err(ConversationError::Unavailable);
            }
            self.publish_preference(&visited);
        } else {
            self.publish_preference(&[]);
        }
        Ok(PreferenceUpdate::Stored(stored))
    }

    /// An allow changed what a followed transcript shows. Every subscriber
    /// re-reads the preference and whether a row is auto-resuming.
    fn publish_preference(&self, visited: &[ConversationId]) {
        self.inner.live_changes.publish_followed();
        for id in visited {
            self.inner.live_changes.publish(id);
        }
        self.inner.schedule_wakeup.notify_waiters();
    }

    /// Apply [`plan_continuation`] and return its error. An applied model
    /// change and a delivery both wait on this. A background settle logs it.
    pub(crate) async fn reconcile_usage_limit_schedule(
        &self,
        id: &ConversationId,
    ) -> Result<(), ConversationError> {
        let Some(store) = self.scheduled_messages() else {
            return Ok(());
        };
        self.apply_usage_limit_plan(id, &store, false, ScheduleActor::system())
            .await
    }

    /// Apply [`plan_continuation`] to one conversation's saved history.
    ///
    /// A plan that does not commit is a reconcile debt with a next attempt.
    /// Catch-up being complete does not drop it. The delivery loop retries
    /// the plan at that time. A debt the database refuses stays in this
    /// process until a later pass writes it or the plan commits.
    pub(crate) async fn consider_usage_limit(
        &self,
        id: &ConversationId,
        enabled_by_person: bool,
        actor: ScheduleActor,
    ) {
        let Some(store) = self.scheduled_messages() else {
            return;
        };
        let conversation = id.to_string();
        if let Err(error) = self
            .apply_usage_limit_plan(id, &store, enabled_by_person, actor)
            .await
        {
            let wait = self.note_reconcile_failure(&store, &conversation).await;
            tracing::warn!(
                conversation_id = %id,
                %error,
                retry_ms = wait,
                "a usage-limit continuation could not be scheduled"
            );
        } else {
            self.forget_reconcile_debt(&store, &conversation).await;
        }
    }

    async fn note_reconcile_failure(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        conversation: &str,
    ) -> u64 {
        let now = self.inner.clock.unix_milliseconds();
        let previous = self.reconcile_failure_count(store, conversation).await;
        let failures = previous.saturating_add(1);
        let wait = attempt_backoff(failures);
        let retry_at = now.saturating_add(wait);
        if store
            .note_reconcile_retry(conversation, failures, retry_at)
            .await
            .is_ok()
        {
            self.forget_memory_reconcile(conversation);
        } else {
            self.remember_reconcile_retry(conversation, failures, retry_at);
        }
        self.inner.schedule_wakeup.notify_waiters();
        wait
    }

    async fn forget_reconcile_debt(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        conversation: &str,
    ) {
        if store.clear_reconcile_retry(conversation).await.is_err() {
            tracing::warn!(
                conversation,
                "a committed usage-limit plan could not clear its retry"
            );
        }
        self.forget_memory_reconcile(conversation);
    }

    async fn reconcile_failure_count(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        conversation: &str,
    ) -> u32 {
        if let Ok(rows) = store.reconcile_retries().await {
            if let Some(row) = rows
                .into_iter()
                .find(|row| row.conversation == conversation)
            {
                return row.failures;
            }
        }
        self.memory_reconcile(conversation)
            .map(|(failures, _)| failures)
            .unwrap_or(0)
    }

    fn memory_reconcile(&self, conversation: &str) -> Option<(u32, u64)> {
        self.inner
            .reconcile_retry_owed
            .lock()
            .expect("reconcile retry")
            .get(conversation)
            .copied()
    }

    fn memory_reconcile_debts(&self) -> Vec<ReconcileRetry> {
        self.inner
            .reconcile_retry_owed
            .lock()
            .expect("reconcile retry")
            .iter()
            .map(|(conversation, (failures, retry_at_ms))| ReconcileRetry {
                conversation: conversation.clone(),
                failures: *failures,
                retry_at_ms: *retry_at_ms,
            })
            .collect()
    }

    fn remember_reconcile_retry(&self, conversation: &str, failures: u32, retry_at_ms: u64) {
        self.inner
            .reconcile_retry_owed
            .lock()
            .expect("reconcile retry")
            .insert(conversation.to_owned(), (failures, retry_at_ms));
    }

    fn forget_memory_reconcile(&self, conversation: &str) {
        self.inner
            .reconcile_retry_owed
            .lock()
            .expect("reconcile retry")
            .remove(conversation);
    }

    /// Retry reconcile debts whose time has arrived. The return is how long
    /// until the soonest debt that is still ahead, or [`SCHEDULE_RETRY_MS`]
    /// when the debt could not be read or written.
    async fn retry_due_reconciles(&self) -> Option<u64> {
        let store = self.scheduled_messages()?;
        let now = self.inner.clock.unix_milliseconds();
        let (mut debts, durable_failed) = match store.reconcile_retries().await {
            Ok(rows) => (rows, false),
            Err(error) => {
                tracing::warn!(%error, "usage-limit reconcile debts could not be read");
                (Vec::new(), true)
            }
        };
        for owed in self.memory_reconcile_debts() {
            if !debts
                .iter()
                .any(|row| row.conversation == owed.conversation)
            {
                debts.push(owed);
            }
        }
        let mut soonest = durable_failed.then_some(SCHEDULE_RETRY_MS);
        for debt in debts {
            if debt.retry_at_ms > now {
                let wait = debt.retry_at_ms - now;
                soonest = Some(soonest.map_or(wait, |current| current.min(wait)));
                continue;
            }
            let Ok(id) = ConversationId::new(&debt.conversation) else {
                self.forget_reconcile_debt(&store, &debt.conversation).await;
                continue;
            };
            match self
                .apply_usage_limit_plan(&id, &store, false, ScheduleActor::system())
                .await
            {
                Ok(()) => {
                    if store
                        .clear_reconcile_retry(&debt.conversation)
                        .await
                        .is_err()
                    {
                        soonest =
                            Some(soonest.map_or(SCHEDULE_RETRY_MS, |current| {
                                current.min(SCHEDULE_RETRY_MS)
                            }));
                    }
                    self.forget_memory_reconcile(&debt.conversation);
                }
                Err(error) => {
                    let failures = debt.failures.saturating_add(1);
                    let wait = attempt_backoff(failures);
                    let retry_at = now.saturating_add(wait);
                    if store
                        .note_reconcile_retry(&debt.conversation, failures, retry_at)
                        .await
                        .is_ok()
                    {
                        self.forget_memory_reconcile(&debt.conversation);
                    } else {
                        self.remember_reconcile_retry(&debt.conversation, failures, retry_at);
                    }
                    soonest = Some(soonest.map_or(wait, |current| current.min(wait)));
                    tracing::warn!(
                        conversation = %debt.conversation,
                        %error,
                        retry_ms = wait,
                        failures,
                        "a usage-limit continuation will be planned again"
                    );
                }
            }
        }
        soonest
    }

    async fn apply_usage_limit_plan(
        &self,
        id: &ConversationId,
        store: &Arc<dyn ScheduledMessageStore>,
        enabled_by_person: bool,
        actor: ScheduleActor,
    ) -> Result<(), ConversationError> {
        // The fence generation is read before the session. A generation
        // attached after the session read could name a newer fence than the
        // history that read returned.
        let generation = store.history_generation(&id.to_string()).await?;
        let Some(mut facts) = self.usage_limit_facts(id).await? else {
            return Ok(());
        };
        if !facts.history.incarnation.is_empty() {
            facts.history.generation = generation;
        }
        let standing = store.preference().await?;
        let open = store.open(&id.to_string()).await?;
        let open_view = open
            .as_ref()
            .map(|row| crate::conversation::domain::OpenContinuation {
                causal_execution: row.causal_execution.clone(),
                due_at_ms: row.due_at_ms,
                delivering: row.delivering,
                agent: row.agent.clone(),
                model: row.model.clone(),
            });
        let now_ms = self.inner.clock.unix_milliseconds();
        let history = facts.history.clone();
        let decided = crate::conversation::domain::plan_continuation(
            standing.enabled,
            now_ms,
            &facts.agent,
            &facts.model,
            facts.observed.as_ref(),
            open_view.as_ref(),
            enabled_by_person,
        );
        match decided {
            ContinuationPlan::Hold => {
                // No row was kept. Seal this generation and dismiss a row a
                // stale accept may have inserted after the plan was read.
                self.seal_history(
                    store,
                    &facts.conversation,
                    &history,
                    open.is_none(),
                    actor.clone(),
                    now_ms,
                )
                .await?;
            }
            ContinuationPlan::Accept { due_at_ms, cause } => {
                self.accept_usage_limit(store, &facts, due_at_ms, cause, actor, standing)
                    .await?;
            }
            ContinuationPlan::MoveDue { due_at_ms } => {
                self.seal_history(
                    store,
                    &facts.conversation,
                    &history,
                    false,
                    actor.clone(),
                    now_ms,
                )
                .await?;
                store
                    .move_due(&id.to_string(), &facts_causal(&facts), due_at_ms, now_ms)
                    .await?;
            }
            ContinuationPlan::Defer { due_at_ms } => {
                self.seal_history(
                    store,
                    &facts.conversation,
                    &history,
                    false,
                    actor.clone(),
                    now_ms,
                )
                .await?;
                store
                    .defer_claim(&id.to_string(), &facts_causal(&facts), due_at_ms, now_ms)
                    .await?;
            }
            ContinuationPlan::Cancel { cause } => {
                self.seal_history(
                    store,
                    &facts.conversation,
                    &history,
                    false,
                    actor.clone(),
                    now_ms,
                )
                .await?;
                store
                    .cancel_open(&id.to_string(), cause, actor, now_ms)
                    .await?;
            }
            ContinuationPlan::Switch {
                cancel,
                due_at_ms,
                cause,
            } => {
                self.seal_history(
                    store,
                    &facts.conversation,
                    &history,
                    false,
                    actor.clone(),
                    now_ms,
                )
                .await?;
                store
                    .cancel_open(&id.to_string(), cancel, actor.clone(), now_ms)
                    .await?;
                self.accept_usage_limit(store, &facts, due_at_ms, cause, actor, standing)
                    .await?;
            }
        }
        if !matches!(decided, ContinuationPlan::Hold) {
            self.inner.schedule_wakeup.notify_waiters();
            self.inner.live_changes.publish(id);
        }
        Ok(())
    }

    async fn accept_usage_limit(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        facts: &UsageLimitFacts,
        due_at_ms: u64,
        cause: ContinuationCause,
        actor: ScheduleActor,
        standing: PreferenceReading,
    ) -> Result<(), ConversationError> {
        let Some(observed) = facts.observed.as_ref() else {
            return Ok(());
        };
        let Some(prompt) = facts.prompt.clone() else {
            return Ok(());
        };
        let at_ms = self.inner.clock.unix_milliseconds();
        let execution_id = ExecutionId::new(uuid::Uuid::new_v4().to_string())
            .map_err(|_| ConversationError::InvalidInput)?;
        store
            .accept(
                ScheduleDraft {
                    conversation: facts.conversation.clone(),
                    causal_execution: observed.causal_execution.clone(),
                    execution_id: execution_id.as_str().to_owned(),
                    due_at_ms,
                    cause,
                    actor,
                    agent: facts.agent.clone(),
                    model: facts.model.clone(),
                    prompt,
                    organization_id: facts.organization_id.clone(),
                    principal_id: facts.principal_id.clone(),
                    surface_id: facts.surface_id.clone(),
                    action_id: facts.action_id.clone(),
                    initiation: facts.initiation.clone(),
                    grant_id: facts.grant_id.clone(),
                    policy_revision: facts.policy_revision,
                    tools_checked: facts.tools_checked,
                    preference_revision: standing.revision,
                    history: facts.history.clone(),
                },
                at_ms,
            )
            .await?;
        Ok(())
    }

    /// Record the history this plan observed, when that read had a generation.
    /// An empty incarnation was not a committed history and does not move the fence.
    async fn seal_history(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        conversation: &str,
        history: &HistoryGeneration,
        dismiss_open: bool,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> Result<(), ConversationError> {
        if history.incarnation.is_empty() {
            return Ok(());
        }
        store
            .note_history(conversation, history, dismiss_open, actor, at_ms)
            .await?;
        Ok(())
    }

    async fn usage_limit_facts(
        &self,
        id: &ConversationId,
    ) -> Result<Option<UsageLimitFacts>, ConversationError> {
        let Some(record) = super::load_conversation(self.inner.metadata.as_ref(), id).await? else {
            return Ok(None);
        };
        let Some(agent) = record.agent() else {
            return Ok(Some(UsageLimitFacts {
                conversation: id.to_string(),
                agent: String::new(),
                model: record.model().as_str().to_owned(),
                observed: None,
                prompt: None,
                organization_id: record.organization().as_str().to_owned(),
                principal_id: record.owner().as_str().to_owned(),
                surface_id: String::new(),
                action_id: String::new(),
                initiation: "owner".into(),
                grant_id: None,
                policy_revision: None,
                tools_checked: false,
                history: HistoryGeneration {
                    incarnation: String::new(),
                    position: 0,
                    generation: 0,
                },
            }));
        };
        let session = super::conversation_session(id);
        let Some(committed) = self.inner.storage.read_committed(session).await? else {
            return Ok(None);
        };
        let Some(snapshot) = committed.snapshot() else {
            return Ok(None);
        };
        let classified = snapshot
            .invocations
            .iter()
            .map(invocation_limit)
            .collect::<Vec<_>>();
        let observed_invocation = crate::conversation::domain::observed_limit_index(&classified)
            .map(|index| &snapshot.invocations[index]);
        let observed = observed_invocation.map(|invocation| {
            let limit = invocation
                .result
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .and_then(nessa_sdk::application::agent_execution::agents::AgentError::usage_limit);
            crate::conversation::domain::ObservedUsageLimit {
                causal_execution: invocation.request.execution_id.as_str().to_owned(),
                resets_at_ms: limit.and_then(|limit| {
                    (limit.recovery()
                        == nessa_sdk::application::agent_execution::agents::UsageLimitRecovery::TimedReset)
                        .then_some(limit.resets_at_ms())
                        .flatten()
                }),
                agent: agent.name().to_owned(),
                model: record.model().as_str().to_owned(),
            }
        });
        let (
            surface_id,
            action_id,
            initiation,
            grant_id,
            policy_revision,
            tools_checked,
            principal,
            prompt,
        ) = if let Some(invocation) = observed_invocation {
            let (initiation, grant_id, policy_revision, tools_checked) =
                initiation_fields(&invocation.initiation);
            (
                invocation.actor.surface_id().to_owned(),
                invocation.actor.request_id().to_owned(),
                initiation,
                grant_id,
                policy_revision,
                tools_checked,
                invocation.actor.principal_id().to_owned(),
                Some(continuation_prompt(
                    invocation.request.user_message.text_str(),
                    self.inner.limits.max_input_bytes,
                )),
            )
        } else {
            (
                String::new(),
                String::new(),
                "owner".into(),
                None,
                None,
                false,
                record.owner().as_str().to_owned(),
                None,
            )
        };
        Ok(Some(UsageLimitFacts {
            conversation: id.to_string(),
            agent: agent.name().to_owned(),
            model: record.model().as_str().to_owned(),
            observed,
            prompt,
            organization_id: record.organization().as_str().to_owned(),
            principal_id: principal,
            surface_id,
            action_id,
            initiation,
            grant_id,
            policy_revision,
            tools_checked,
            history: HistoryGeneration {
                incarnation: committed.incarnation().to_owned(),
                position: committed.position(),
                generation: 0,
            },
        }))
    }

    /// Every catalogue owner, every page. `visited` collects the conversations
    /// already planned, including when a later page fails.
    async fn schedule_every_owner(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        enabled_by_person: bool,
        actor: ScheduleActor,
        visited: &mut Vec<ConversationId>,
    ) -> Result<(), ConversationError> {
        let owners = self.inner.listing.owners().await?;
        for (organization, principal) in owners {
            let mut cursor = None::<crate::conversation::application::ObservationCursor>;
            loop {
                let page = self
                    .inner
                    .listing
                    .observe(&organization, &principal, false, cursor.clone())
                    .await?;
                let complete = page.complete;
                let next = page.cursor;
                let mut page_ids = Vec::new();
                for row in page.conversations {
                    let id = row.conversation.id().clone();
                    self.apply_usage_limit_plan(&id, store, enabled_by_person, actor.clone())
                        .await?;
                    page_ids.push(id);
                }
                if fold_catalogue_page(&mut cursor, complete, next, page_ids, visited)? {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Deliver continuations that are due. Returns how long to wait for the
    /// next one, when a scheduled row is still ahead of the clock. A store
    /// error waits [`SCHEDULE_RETRY_MS`] and passes again. `None` is an
    /// unbound store, or a schedule with nothing due and no live lease.
    pub async fn deliver_due_scheduled(&self) -> Option<u64> {
        let store = self.scheduled_messages()?;
        let now = self.inner.clock.unix_milliseconds();
        let preference = match store.preference().await {
            Ok(preference) => preference.enabled,
            Err(_) => return Some(SCHEDULE_RETRY_MS),
        };
        if !preference {
            if store.retire_unfenced_submissions(now).await.is_err() {
                return Some(SCHEDULE_RETRY_MS);
            }
            return match store.next_live_fence(now).await {
                Ok(Some(until)) => Some(until.saturating_sub(now).max(1)),
                Ok(None) => None,
                Err(_) => Some(SCHEDULE_RETRY_MS),
            };
        }
        let rows = match store.claim_due(now, 8).await {
            Ok(rows) => rows,
            Err(_) => return Some(SCHEDULE_RETRY_MS),
        };
        let mut retry_unrecorded = false;
        for row in rows {
            if self.deliver_scheduled(store.clone(), row).await {
                retry_unrecorded = true;
            }
        }
        if retry_unrecorded {
            return Some(SCHEDULE_RETRY_MS);
        }
        let next = match store.next_due(self.inner.clock.unix_milliseconds()).await {
            Ok(next) => next,
            Err(_) => return Some(SCHEDULE_RETRY_MS),
        };
        let next = next?;
        let now = self.inner.clock.unix_milliseconds();
        Some(next.saturating_sub(now).max(1))
    }

    /// Run delivery until the gateway retires.
    ///
    /// The wakeup is registered before the catch-up scan and before each
    /// schedule read. Catch-up is complete only after every catalogue owner
    /// has been traversed while the preference is on, or when the preference
    /// is off. A failed scan leaves it pending. The next pass tries again
    /// after [`attempt_backoff`]. A preference read that fails leaves it
    /// pending the same way. A reconcile debt whose plan did not commit is
    /// retried at its own time, including when catch-up is already complete.
    pub async fn run_scheduled_messages(&self) {
        let mut caught_up = self.scheduled_messages().is_none();
        let mut catch_up_failures = 0_u32;
        loop {
            if self.inner.retirement.get().is_some() {
                return;
            }
            let wakeup = self.inner.schedule_wakeup.notified();
            let mut catch_up_wait = None;
            if !caught_up {
                if let Some(store) = self.scheduled_messages() {
                    match store.preference().await {
                        Ok(reading) => {
                            if reading.enabled {
                                let mut visited = Vec::new();
                                match self
                                    .schedule_every_owner(
                                        &store,
                                        false,
                                        ScheduleActor::system(),
                                        &mut visited,
                                    )
                                    .await
                                {
                                    Ok(()) => {
                                        caught_up = true;
                                        catch_up_failures = 0;
                                    }
                                    Err(error) => {
                                        catch_up_failures = catch_up_failures.saturating_add(1);
                                        let retry = attempt_backoff(catch_up_failures);
                                        catch_up_wait = Some(retry);
                                        tracing::warn!(
                                            %error,
                                            retry_ms = retry,
                                            "a usage-limit continuation scan did not finish"
                                        );
                                    }
                                }
                                for id in &visited {
                                    self.inner.live_changes.publish(id);
                                }
                            } else {
                                caught_up = true;
                                catch_up_failures = 0;
                            }
                        }
                        Err(error) => {
                            catch_up_failures = catch_up_failures.saturating_add(1);
                            let retry = attempt_backoff(catch_up_failures);
                            catch_up_wait = Some(retry);
                            tracing::warn!(
                                %error,
                                retry_ms = retry,
                                "the usage-limit preference could not be read; the catch-up scan waits"
                            );
                        }
                    }
                }
            }
            let reconcile_wait = self.retry_due_reconciles().await;
            let wait = next_pass_wait(
                self.deliver_due_scheduled().await,
                catch_up_wait,
                reconcile_wait,
            );
            tokio::select! {
                () = async {
                    match wait {
                        Some(wait) => tokio::time::sleep(Duration::from_millis(wait)).await,
                        None => std::future::pending().await,
                    }
                } => {}
                () = wakeup => {}
                () = self.retired() => return,
            }
        }
    }

    async fn deliver_scheduled(
        &self,
        store: Arc<dyn ScheduledMessageStore>,
        row: ScheduledMessage,
    ) -> bool {
        let Ok(id) = ConversationId::new(&row.conversation) else {
            return false;
        };
        // A plan that does not commit leaves the row where it is. Submitting
        // it would deliver a continuation the model change had not cancelled.
        if self
            .apply_usage_limit_plan(&id, &store, false, ScheduleActor::system())
            .await
            .is_err()
        {
            return false;
        }
        // Reconciliation above may cancel or defer while the row is still
        // `delivering`. The transition below is the decision to submit: a
        // cancel, model change, or later reset that wins first leaves this
        // false, and one that arrives after cannot undo it.
        let now = self.inner.clock.unix_milliseconds();
        let Ok(true) = store.begin_submit(&row.execution_id, now).await else {
            return false;
        };
        let lease_until = now.saturating_add(FENCE_LEASE_MS);
        let unrecorded = self
            .finish_fenced_submit(store.clone(), id, &row, lease_until)
            .await;
        let _ = store.release_submit(&row.execution_id).await;
        unrecorded
    }

    async fn finish_fenced_submit(
        &self,
        store: Arc<dyn ScheduledMessageStore>,
        id: ConversationId,
        row: &ScheduledMessage,
        lease_until: u64,
    ) -> bool {
        let lost = Arc::new(AtomicBool::new(false));
        let renew = {
            let store = store.clone();
            let execution_id = row.execution_id.clone();
            let clock = self.inner.clock.clone();
            let lost = lost.clone();
            tokio::spawn(async move {
                renew_while_held(store, execution_id, clock, lost, lease_until).await;
            })
        };
        let _renew = StopRenew(renew);
        // The lease this process wrote has already expired. Do not start a
        // submit, and do not record an outcome on a row another process may
        // adopt. A saved execution is recorded by the process that adopts it.
        if self.inner.clock.unix_milliseconds() >= lease_until {
            lost.store(true, Ordering::SeqCst);
            return false;
        }
        match self.execution_saved(&id, &row.execution_id).await {
            Ok(true) => {
                let now = self.inner.clock.unix_milliseconds();
                let _ = store
                    .record_outcome(
                        &row.execution_id,
                        ScheduleSettlement::Delivered,
                        "delivered",
                        now,
                    )
                    .await;
                return false;
            }
            Ok(false) => {}
            Err(error) => {
                // The invocation may already be saved. An unread session is
                // not absence: submitting would see Attempted and record
                // unconfirmed for a turn that is still running.
                tracing::warn!(
                    execution_id = %row.execution_id,
                    %error,
                    "the saved execution could not be read; the submission stays and will be read again"
                );
                return true;
            }
        }
        let Some(commands) = self.commands() else {
            if lost.load(Ordering::SeqCst) {
                return false;
            }
            return !self.hold_submit_retry(&store, row).await;
        };
        // A stored caller or grant that cannot be rebuilt will never pass
        // submit. Refuse it once. The row stays out of `delivering`, so the
        // next pass adopts `submitting` only until this refusal is recorded.
        let (caller, initiation) = match (schedule_caller(row), continuation_initiation(row)) {
            (Ok(caller), Ok(initiation)) => (caller, initiation),
            _ => {
                if lost.load(Ordering::SeqCst) {
                    return false;
                }
                let now = self.inner.clock.unix_milliseconds();
                let _ = store
                    .record_outcome(
                        &row.execution_id,
                        ScheduleSettlement::Refused,
                        "revoked",
                        now,
                    )
                    .await;
                return false;
            }
        };
        // The store said this process no longer owns the row, or the lease it
        // wrote has expired. Do not start a second effect, and do not move a
        // retry time on a row we do not own.
        if lost.load(Ordering::SeqCst) {
            return false;
        }
        match self
            .submit_command(
                commands,
                id,
                caller,
                initiation,
                false,
                row.execution_id.clone(),
                super::SubmittedMessage {
                    text: row.prompt.clone(),
                    ..Default::default()
                },
                super::SubmissionMode::Queue,
            )
            .await
        {
            Ok(_) => {
                if lost.load(Ordering::SeqCst) {
                    return false;
                }
                let now = self.inner.clock.unix_milliseconds();
                let _ = store
                    .record_outcome(
                        &row.execution_id,
                        ScheduleSettlement::Delivered,
                        "delivered",
                        now,
                    )
                    .await;
                false
            }
            Err(error) => {
                if lost.load(Ordering::SeqCst) {
                    return false;
                }
                if let Some((outcome, cause)) = settled_outcome(&error) {
                    let now = self.inner.clock.unix_milliseconds();
                    let _ = store
                        .record_outcome(&row.execution_id, outcome, cause, now)
                        .await;
                    false
                } else {
                    !self.hold_submit_retry(&store, row).await
                }
            }
        }
    }

    /// Write the next attempt on a submitting row this process owns.
    /// True when that timestamp and its audit committed.
    async fn hold_submit_retry(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        row: &ScheduledMessage,
    ) -> bool {
        let now = self.inner.clock.unix_milliseconds();
        let failures = row.retry_failures.saturating_add(1);
        let retry_at = now.saturating_add(attempt_backoff(failures));
        match store
            .defer_submit(&row.execution_id, failures, retry_at, now)
            .await
        {
            Ok(true) => true,
            Ok(false) => {
                tracing::warn!(
                    execution_id = %row.execution_id,
                    "a retryable scheduled submission could not be deferred"
                );
                false
            }
            Err(error) => {
                tracing::warn!(
                    execution_id = %row.execution_id,
                    %error,
                    "a retryable scheduled submission could not be deferred"
                );
                false
            }
        }
    }

    async fn execution_saved(
        &self,
        id: &ConversationId,
        execution_id: &str,
    ) -> Result<bool, ConversationError> {
        let session = super::conversation_session(id);
        let Some(committed) = self.inner.storage.read_committed(session).await? else {
            return Ok(false);
        };
        Ok(committed.snapshot().is_some_and(|snapshot| {
            snapshot
                .invocations
                .iter()
                .any(|invocation| invocation.request.execution_id.as_str() == execution_id)
        }))
    }
}

/// How long to wait before the next failed attempt.
///
/// The first failure waits [`SCHEDULE_RETRY_MS`]. Each later failure doubles
/// that wait, up to [`CATCH_UP_RETRY_MAX_MS`]. Catch-up, a reconcile debt,
/// and a retryable submit all use this curve.
fn attempt_backoff(failures: u32) -> u64 {
    let mut wait = SCHEDULE_RETRY_MS;
    for _ in 0..failures.saturating_sub(1) {
        let doubled = wait.saturating_mul(2);
        if doubled >= CATCH_UP_RETRY_MAX_MS {
            return CATCH_UP_RETRY_MAX_MS;
        }
        wait = doubled;
    }
    wait
}

/// The sooner of a delivery wait, a pending catch-up retry, and a reconcile debt.
///
/// A catch-up or a failed settlement that is still owed must not park on an
/// empty schedule. A due row must not wait out the whole backoff.
fn next_pass_wait(
    delivery: Option<u64>,
    catch_up: Option<u64>,
    reconcile: Option<u64>,
) -> Option<u64> {
    [delivery, catch_up, reconcile].into_iter().flatten().min()
}

/// Renew `execution_id` until this process no longer holds a live lease.
///
/// `deadline` is the `fence_until_ms` this process last wrote. `Ok(true)`
/// waits [`FENCE_RENEW_EVERY_MS`] and renews again, and the deadline becomes
/// the instant that renew wrote. `Err` waits [`FENCE_RENEW_RETRY_MS`] and
/// renews again only while `deadline` is still ahead. Once the clock reaches
/// `deadline`, or `renew_fence` returns false, `lost` is set and renewal
/// stops. An expired lease is not retried and is not extended.
async fn renew_while_held(
    store: Arc<dyn ScheduledMessageStore>,
    execution_id: String,
    clock: Arc<dyn Clock>,
    lost: Arc<AtomicBool>,
    mut deadline: u64,
) {
    loop {
        if clock.unix_milliseconds() >= deadline {
            lost.store(true, Ordering::SeqCst);
            return;
        }
        let until_renew = deadline
            .saturating_sub(clock.unix_milliseconds())
            .min(FENCE_RENEW_EVERY_MS);
        tokio::time::sleep(Duration::from_millis(until_renew)).await;
        if clock.unix_milliseconds() >= deadline {
            lost.store(true, Ordering::SeqCst);
            return;
        }
        loop {
            let now = clock.unix_milliseconds();
            if now >= deadline {
                lost.store(true, Ordering::SeqCst);
                return;
            }
            match store.renew_fence(&execution_id, now).await {
                Ok(true) => {
                    deadline = now.saturating_add(FENCE_LEASE_MS);
                    break;
                }
                Ok(false) => {
                    lost.store(true, Ordering::SeqCst);
                    return;
                }
                Err(error) => {
                    tracing::debug!(
                        %error,
                        %execution_id,
                        "the submitting lease could not be renewed; retrying"
                    );
                    if clock.unix_milliseconds() >= deadline {
                        lost.store(true, Ordering::SeqCst);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(FENCE_RENEW_RETRY_MS)).await;
                }
            }
        }
    }
}

/// Aborts the lease renewal when the submit returns, including on an early return.
struct StopRenew(tokio::task::JoinHandle<()>);

impl Drop for StopRenew {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct UsageLimitFacts {
    conversation: String,
    agent: String,
    model: String,
    observed: Option<crate::conversation::domain::ObservedUsageLimit>,
    prompt: Option<String>,
    organization_id: String,
    principal_id: String,
    surface_id: String,
    action_id: String,
    initiation: String,
    grant_id: Option<String>,
    policy_revision: Option<u64>,
    tools_checked: bool,
    history: HistoryGeneration,
}

/// Read one catalogue page into `out`. `Ok(true)` is the last page. An
/// unfinished page with no new cursor is an error, so the walk cannot stop
/// early and report that every conversation was seen.
fn fold_catalogue_page<T, C: Clone + PartialEq>(
    cursor: &mut Option<C>,
    complete: bool,
    next: Option<C>,
    items: impl IntoIterator<Item = T>,
    out: &mut Vec<T>,
) -> Result<bool, ConversationError> {
    out.extend(items);
    if complete {
        return Ok(true);
    }
    let next = next.ok_or(ConversationError::Unavailable)?;
    if cursor.as_ref() == Some(&next) {
        return Err(ConversationError::Unavailable);
    }
    *cursor = Some(next);
    Ok(false)
}

fn facts_causal(facts: &UsageLimitFacts) -> String {
    facts
        .observed
        .as_ref()
        .map(|observed| observed.causal_execution.clone())
        .unwrap_or_default()
}

pub(crate) fn continuation_prompt(user_text: &str, max_bytes: usize) -> String {
    const NOTICE: &str =
        "The usage limit reset. Continue the request that was waiting.\n\nThe request was:\n";
    if max_bytes == 0 {
        return String::new();
    }
    // The notice is the prompt when the input bound is shorter than it. The
    // scheduled submission has to pass the same bound, so the prompt never
    // starts longer than `max_bytes`.
    let notice_end = floor_bytes(NOTICE, max_bytes);
    let mut prompt = NOTICE[..notice_end].to_owned();
    let room = max_bytes.saturating_sub(prompt.len());
    if room == 0 {
        return prompt;
    }
    let text = user_text.trim();
    let end = floor_bytes(text, room);
    prompt.push_str(&text[..end]);
    prompt
}

fn floor_bytes(text: &str, max: usize) -> usize {
    if text.len() <= max {
        return text.len();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn invocation_limit(
    invocation: &nessa_sdk::application::agent_execution::sessions::storage::InvocationRecord,
) -> crate::conversation::domain::InvocationLimit {
    use crate::conversation::domain::InvocationLimit;
    match invocation.result.as_ref() {
        None => InvocationLimit::Unsettled,
        Some(result) => match result
            .as_ref()
            .err()
            .and_then(nessa_sdk::application::agent_execution::agents::AgentError::usage_limit)
        {
            Some(_) => InvocationLimit::UsageLimit,
            None => InvocationLimit::Settled,
        },
    }
}

/// Rebuild the initiation a delivery submits under. A grant is checked again
/// inside submit; this only carries the identity the causal turn was admitted with.
pub(crate) fn continuation_initiation(
    row: &ScheduledMessage,
) -> Result<Initiation, ConversationError> {
    if row.initiation == "owner" {
        return Ok(Initiation::owner());
    }
    let grant = GrantInitiator::new(
        row.grant_id.clone().unwrap_or_default(),
        row.policy_revision.unwrap_or(0),
    )
    .map_err(|_| ConversationError::InvalidInput)?;
    Ok(if row.tools_checked {
        Initiation::checked_grant(grant)
    } else {
        Initiation::grant(grant)
    })
}

fn schedule_caller(row: &ScheduledMessage) -> Result<ConversationCaller, ConversationError> {
    Ok(ConversationCaller {
        organization_id: nessa_auth::domain::OrganizationId::new(&row.organization_id)
            .map_err(|_| ConversationError::InvalidInput)?,
        principal_id: nessa_auth::domain::PrincipalId::new(&row.principal_id)
            .map_err(|_| ConversationError::InvalidInput)?,
        surface_id: row.surface_id.clone(),
        action_id: row.execution_id.clone(),
    })
}

fn settled_outcome(
    error: &nessa_sdk::application::agent_execution::commands::MutationFailure<ConversationError>,
) -> Option<(ScheduleSettlement, &'static str)> {
    use nessa_sdk::application::agent_execution::commands::MutationFailure;
    if matches!(error, MutationFailure::Interrupted(_)) {
        return Some((
            ScheduleSettlement::Unconfirmed,
            ContinuationCause::SubmissionInterrupted.as_str(),
        ));
    }
    let MutationFailure::Target(error) = error else {
        return None;
    };
    match error {
        ConversationError::Deleted => Some((ScheduleSettlement::Refused, "deleted")),
        ConversationError::NotFound => Some((ScheduleSettlement::Refused, "not_found")),
        ConversationError::Revoked | ConversationError::RevokedAfterSave => {
            Some((ScheduleSettlement::Refused, "revoked"))
        }
        ConversationError::UnavailableAfterSave => {
            Some((ScheduleSettlement::Refused, "unavailable"))
        }
        _ => None,
    }
}

pub(crate) fn initiation_fields(
    initiation: &Initiation,
) -> (String, Option<String>, Option<u64>, bool) {
    match initiation.initiator() {
        Initiator::Owner => ("owner".into(), None, None, false),
        Initiator::Grant(grant) => (
            "grant".into(),
            Some(grant.grant().to_owned()),
            Some(grant.policy_revision()),
            initiation.tools() == ToolChecking::Checked,
        ),
    }
}

#[cfg(test)]
mod catalogue_pages {
    use super::fold_catalogue_page;
    use crate::conversation::application::ConversationError;

    #[test]
    fn enabling_the_preference_reads_past_the_list_bound() {
        let mut cursor = None;
        let mut ids = Vec::new();
        let first: Vec<_> = (0..500).collect();
        let second: Vec<_> = (500..600).collect();
        assert!(!fold_catalogue_page(&mut cursor, false, Some(1_u32), first, &mut ids).unwrap());
        assert!(!fold_catalogue_page(&mut cursor, false, Some(2), second, &mut ids).unwrap());
        assert!(
            fold_catalogue_page(&mut cursor, true, None, std::iter::empty(), &mut ids).unwrap()
        );
        assert_eq!(ids.len(), 600);
        assert_eq!(ids[0], 0);
        assert_eq!(ids[599], 599);
    }

    #[test]
    fn an_unfinished_catalogue_page_without_a_cursor_fails_the_walk() {
        let mut cursor = None::<u32>;
        let mut ids = Vec::new();
        let error = fold_catalogue_page(&mut cursor, false, None, ["older"], &mut ids);
        assert!(matches!(error, Err(ConversationError::Unavailable)));
    }

    #[test]
    fn a_catalogue_cursor_that_does_not_move_fails_the_walk() {
        let mut cursor = None;
        let mut ids = Vec::new();
        assert!(!fold_catalogue_page(&mut cursor, false, Some(1_u32), ["a"], &mut ids).unwrap());
        let error = fold_catalogue_page(&mut cursor, false, Some(1), ["a"], &mut ids);
        assert!(matches!(error, Err(ConversationError::Unavailable)));
    }

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use nessa_auth::application::ports::{AccessError, Clock};
    use nessa_auth::domain::{OrganizationId, PrincipalId};
    use nessa_sdk::application::agent_execution::commands::{
        CreationFuture, CreationReceipt, CreationStorage, CreationStorageError,
        CreationStorageLease, MutationReceipt,
    };
    use nessa_sdk::application::agent_execution::sessions::{
        SessionStorage, StorageError, StorageFuture,
    };
    use nessa_sdk::domain::agent_execution::sessions::SessionId;

    use super::{
        renew_while_held, PreferenceCommandOutcome, PreferenceCommandReceipt, PreferenceReading,
        PreferenceUpdate, RecordedPreference, ScheduleActor, ScheduleDraft, ScheduleSettlement,
        ScheduledMessage, ScheduledMessageStore, FENCE_LEASE_MS, FENCE_RENEW_EVERY_MS,
        FENCE_RENEW_RETRY_MS, SCHEDULE_RETRY_MS,
    };
    use crate::conversation::application::{
        ConversationCaller, ConversationDependencies, ConversationFuture, ConversationLimits,
        ConversationListing, ConversationModelApplication, ConversationModelRequest,
        ConversationModelRequestState, ConversationModelSelection, ConversationService,
        ListedConversation, ListedConversations, ObservationCursor, ObservedConversations,
        UsageLimitPreferenceDecision, UsageLimitPreferenceFacts, UsageLimitPreferenceGrant,
        UsageLimitPreferencePolicyEvaluator,
    };
    use crate::conversation::domain::ContinuationCause;
    use crate::conversation_test_support::{
        only, AcceptingCreationAudit, AcceptingDeletionAudit, InMemoryStorage, MemoryRepository,
        MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit, TestClock,
        DELETION_BUDGETS,
    };
    use nessa_protocol::conversation::domain::ConversationId;
    use nessa_protocol::conversation::domain::{
        ConversationApprovalMode, ConversationModelId, ConversationSummary,
    };
    use nessa_protocol::conversation::view::ConversationModelChangeEffect;
    use nessa_sdk::application::agent_execution::providers::ProviderIdentity;

    fn caller_as(action: &str) -> ConversationCaller {
        ConversationCaller {
            organization_id: OrganizationId::new("org").unwrap(),
            principal_id: PrincipalId::new("person").unwrap(),
            surface_id: "desktop".into(),
            action_id: action.into(),
        }
    }

    fn caller() -> ConversationCaller {
        caller_as("toggle")
    }

    fn facts(role: &str) -> UsageLimitPreferenceFacts {
        UsageLimitPreferenceFacts {
            principal_id: "person".into(),
            organization_id: "org".into(),
            role: role.into(),
            active: true,
            grants: vec![UsageLimitPreferenceGrant {
                action: "credential.manage".into(),
                resource_id: "gateway".into(),
                organization_id: "org".into(),
            }],
            gateway_id: "gateway".into(),
            gateway_organization: "org".into(),
        }
    }

    fn service(
        listing: Arc<dyn ConversationListing>,
        clock: Arc<dyn Clock>,
    ) -> ConversationService {
        service_over(Arc::new(MemoryRepository::default()), listing, clock)
    }

    fn service_over(
        metadata: Arc<MemoryRepository>,
        listing: Arc<dyn ConversationListing>,
        clock: Arc<dyn Clock>,
    ) -> ConversationService {
        service_stored(metadata, listing, clock, Arc::new(InMemoryStorage::new()))
    }

    fn service_stored(
        metadata: Arc<MemoryRepository>,
        listing: Arc<dyn ConversationListing>,
        clock: Arc<dyn Clock>,
        storage: Arc<dyn SessionStorage>,
    ) -> ConversationService {
        let provider = Arc::new(ProviderFactory::default());
        ConversationService::new(
            ConversationDependencies {
                access_policy: crate::conversation_test_support::access_policy(),
                agents: only(Arc::new(Provider::new(provider))),
                storage,
                metadata,
                mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: None,
                summaries: Arc::new(MemorySummaries::default()),
                listing,
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions: crate::conversation_test_support::claude_erasers(),
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock:
                    crate::conversation_test_support::runtime_message_commit_clock(),
                clock,
                environment: crate::conversation_test_support::in_process_environment(),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap()
    }

    /// Allows an active admin and refuses every other role. The Cedar bundle is
    /// tested beside its policies; this double is what the service is shown.
    struct AdminOnly;

    impl UsageLimitPreferencePolicyEvaluator for AdminOnly {
        fn evaluate(
            &self,
            facts: &UsageLimitPreferenceFacts,
        ) -> Result<UsageLimitPreferenceDecision, AccessError> {
            if facts.role == "admin" && facts.active {
                Ok(UsageLimitPreferenceDecision::Allow {
                    policies: vec!["admin".into()],
                })
            } else {
                Ok(UsageLimitPreferenceDecision::Deny {
                    policies: vec!["member".into()],
                })
            }
        }
    }

    /// Preference changes and outcome timestamps, without a database.
    struct RecordingStore {
        enabled: AtomicBool,
        revision: AtomicU64,
        /// False only for an enable this store has not settled or reverted.
        settled: AtomicBool,
        /// `(enabled, person)` for each change. An unchanged write is absent.
        changes: Mutex<Vec<(bool, bool)>>,
        outcomes: Mutex<Vec<(String, u64)>>,
        /// `(allowed, policy ids)` for each Cedar decision that committed.
        decisions: Mutex<Vec<(bool, Vec<String>)>>,
        fail_decision: AtomicBool,
        submits: AtomicUsize,
        opens: AtomicUsize,
        /// Conversation ids passed to `open`, in order.
        opened: Mutex<Vec<String>>,
        /// While set, `open` fails and a delivery or model reconciliation cannot commit.
        fail_open: AtomicBool,
        /// While set, a delivery pass finds nothing due instead of failing the read.
        idle_delivery: AtomicBool,
        /// `preference` fails this many times, then returns the stored boolean.
        preference_failures: AtomicUsize,
        /// When set, delivery claims `due_row` and a submit retry is held until `retry_at_ms`.
        submit_attempts: AtomicBool,
        due_row: Mutex<Option<ScheduledMessage>>,
        retry_at_ms: AtomicU64,
        retry_failures: AtomicU32,
        /// `(execution id, failures, retry_at_ms)` for each committed submit hold.
        defers: Mutex<Vec<(String, u32, u64)>>,
        reconciles: Mutex<HashMap<String, (u32, u64)>>,
        /// The next reconcile-debt write fails. The service keeps the debt in memory.
        fail_reconcile_note: AtomicBool,
        /// Receipts that committed, keyed by request id.
        commands: Mutex<HashMap<String, PreferenceCommandReceipt>>,
        /// The request that installed the current unsettled enable.
        installed_request: Mutex<String>,
        /// The next settlement loses, as a concurrent disable would.
        lose_settle: AtomicBool,
    }

    impl RecordingStore {
        fn new(enabled: bool) -> Self {
            Self {
                enabled: AtomicBool::new(enabled),
                revision: AtomicU64::new(0),
                settled: AtomicBool::new(true),
                changes: Mutex::new(Vec::new()),
                outcomes: Mutex::new(Vec::new()),
                decisions: Mutex::new(Vec::new()),
                fail_decision: AtomicBool::new(false),
                submits: AtomicUsize::new(0),
                opens: AtomicUsize::new(0),
                opened: Mutex::new(Vec::new()),
                fail_open: AtomicBool::new(true),
                idle_delivery: AtomicBool::new(false),
                preference_failures: AtomicUsize::new(0),
                submit_attempts: AtomicBool::new(false),
                due_row: Mutex::new(None),
                retry_at_ms: AtomicU64::new(0),
                retry_failures: AtomicU32::new(0),
                defers: Mutex::new(Vec::new()),
                reconciles: Mutex::new(HashMap::new()),
                fail_reconcile_note: AtomicBool::new(false),
                commands: Mutex::new(HashMap::new()),
                installed_request: Mutex::new(String::new()),
                lose_settle: AtomicBool::new(false),
            }
        }

        /// Settle `revision` when it is still the enabled preference.
        fn settle_revision(&self, revision: u64) -> bool {
            if self.lose_settle.swap(false, Ordering::SeqCst) {
                return false;
            }
            if self.revision.load(Ordering::SeqCst) == revision
                && self.enabled.load(Ordering::SeqCst)
            {
                self.settled.store(true, Ordering::SeqCst);
                true
            } else {
                false
            }
        }
    }

    fn refused<T: Send + 'static>() -> ConversationFuture<'static, T> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }

    impl ScheduledMessageStore for RecordingStore {
        fn preference(&self) -> ConversationFuture<'_, PreferenceReading> {
            if self.preference_failures.load(Ordering::SeqCst) > 0 {
                self.preference_failures.fetch_sub(1, Ordering::SeqCst);
                return refused();
            }
            let enabled = self.enabled.load(Ordering::SeqCst);
            let revision = self.revision.load(Ordering::SeqCst);
            Box::pin(async move { Ok(PreferenceReading { enabled, revision }) })
        }
        fn set_preference(
            &self,
            enabled: bool,
            actor: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            let was = self.enabled.swap(enabled, Ordering::SeqCst);
            if was != enabled {
                self.revision.fetch_add(1, Ordering::SeqCst);
                self.settled.store(true, Ordering::SeqCst);
                self.changes.lock().unwrap().push((enabled, actor.person));
            }
            Box::pin(async move { Ok(enabled) })
        }
        fn open(&self, conversation: &str) -> ConversationFuture<'_, Option<ScheduledMessage>> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            self.opened.lock().unwrap().push(conversation.to_owned());
            if self.fail_open.load(Ordering::SeqCst) {
                refused()
            } else {
                Box::pin(async { Ok(None) })
            }
        }
        fn accept(
            &self,
            _: ScheduleDraft,
            _: u64,
        ) -> ConversationFuture<'_, Option<ScheduledMessage>> {
            refused()
        }
        fn move_due(&self, _: &str, _: &str, _: u64, _: u64) -> ConversationFuture<'_, bool> {
            refused()
        }
        fn defer_claim(&self, _: &str, _: &str, _: u64, _: u64) -> ConversationFuture<'_, bool> {
            refused()
        }
        fn cancel_open(
            &self,
            _: &str,
            _: ContinuationCause,
            _: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            refused()
        }
        fn claim_due(&self, now: u64, _: usize) -> ConversationFuture<'_, Vec<ScheduledMessage>> {
            if self.submit_attempts.load(Ordering::SeqCst) {
                let retry = self.retry_at_ms.load(Ordering::SeqCst);
                if retry > now {
                    return Box::pin(async { Ok(Vec::new()) });
                }
                let row = self.due_row.lock().unwrap().clone().map(|mut row| {
                    row.retry_failures = self.retry_failures.load(Ordering::SeqCst);
                    row
                });
                return Box::pin(async move { Ok(row.into_iter().collect()) });
            }
            if self.idle_delivery.load(Ordering::SeqCst) {
                Box::pin(async { Ok(Vec::new()) })
            } else {
                refused()
            }
        }
        fn next_due(&self, now: u64) -> ConversationFuture<'_, Option<u64>> {
            if self.submit_attempts.load(Ordering::SeqCst) {
                let retry = self.retry_at_ms.load(Ordering::SeqCst);
                let waiting = self.due_row.lock().unwrap().is_some();
                return Box::pin(async move {
                    if retry > now {
                        Ok(Some(retry))
                    } else if waiting {
                        Ok(Some(now))
                    } else {
                        Ok(None)
                    }
                });
            }
            if self.idle_delivery.load(Ordering::SeqCst) {
                Box::pin(async { Ok(None) })
            } else {
                refused()
            }
        }
        fn begin_submit(&self, _: &str, now: u64) -> ConversationFuture<'_, bool> {
            if self.submit_attempts.load(Ordering::SeqCst) {
                let retry = self.retry_at_ms.load(Ordering::SeqCst);
                if retry > now {
                    return Box::pin(async { Ok(false) });
                }
                self.submits.fetch_add(1, Ordering::SeqCst);
                return Box::pin(async { Ok(true) });
            }
            self.submits.fetch_add(1, Ordering::SeqCst);
            refused()
        }
        fn renew_fence(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
            Box::pin(async { Ok(false) })
        }
        fn release_submit(&self, _: &str) -> ConversationFuture<'_, ()> {
            refused()
        }
        fn retire_unfenced_submissions(&self, _: u64) -> ConversationFuture<'_, usize> {
            refused()
        }
        fn next_live_fence(&self, _: u64) -> ConversationFuture<'_, Option<u64>> {
            Box::pin(async { Ok(None) })
        }
        fn preference_command(
            &self,
            request_id: &str,
        ) -> ConversationFuture<'_, Option<PreferenceCommandReceipt>> {
            let saved = self.commands.lock().unwrap().get(request_id).cloned();
            Box::pin(async move { Ok(saved) })
        }
        fn record_preference_decision(
            &self,
            allowed: bool,
            enabled: bool,
            policies: &[String],
            actor: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, RecordedPreference> {
            if self.fail_decision.swap(false, Ordering::SeqCst) {
                return Box::pin(async { Err(ConversationError::Metadata) });
            }
            let Some(request_id) = actor.action_id.clone().filter(|value| !value.is_empty()) else {
                return Box::pin(async { Err(ConversationError::InvalidInput) });
            };
            let organization_id = actor.organization_id.clone().unwrap_or_default();
            let principal_id = actor.principal_id.clone().unwrap_or_default();
            let surface_id = actor.surface_id.clone().unwrap_or_default();
            if organization_id.is_empty() || principal_id.is_empty() || surface_id.is_empty() {
                return Box::pin(async { Err(ConversationError::InvalidInput) });
            }
            if let Some(saved) = self.commands.lock().unwrap().get(&request_id).cloned() {
                if saved.organization_id != organization_id
                    || saved.principal_id != principal_id
                    || saved.surface_id != surface_id
                    || saved.enabled != enabled
                {
                    return Box::pin(async { Err(ConversationError::RequestConflict) });
                }
                let revision = self.revision.load(Ordering::SeqCst);
                let update = match saved.outcome {
                    PreferenceCommandOutcome::Stored(value) => PreferenceUpdate::Stored(value),
                    PreferenceCommandOutcome::Refused { policies } => {
                        PreferenceUpdate::Refused { policies }
                    }
                    PreferenceCommandOutcome::Failed => {
                        return Box::pin(async { Err(ConversationError::Unavailable) });
                    }
                };
                return Box::pin(async move {
                    Ok(RecordedPreference {
                        update,
                        revision,
                        installed_enable: false,
                        replayed: true,
                    })
                });
            }
            self.decisions
                .lock()
                .unwrap()
                .push((allowed, policies.to_vec()));
            let (update, installed_enable) = if !allowed {
                let revision = self.revision.load(Ordering::SeqCst);
                let refused = policies.to_vec();
                self.commands.lock().unwrap().insert(
                    request_id,
                    PreferenceCommandReceipt {
                        organization_id,
                        principal_id,
                        surface_id,
                        enabled,
                        outcome: PreferenceCommandOutcome::Refused {
                            policies: refused.clone(),
                        },
                    },
                );
                return Box::pin(async move {
                    Ok(RecordedPreference {
                        update: PreferenceUpdate::Refused { policies: refused },
                        revision,
                        installed_enable: false,
                        replayed: false,
                    })
                });
            } else {
                let was = self.enabled.swap(enabled, Ordering::SeqCst);
                let installed_enable = !was && enabled;
                if was != enabled {
                    self.revision.fetch_add(1, Ordering::SeqCst);
                    self.settled.store(!installed_enable, Ordering::SeqCst);
                    self.changes.lock().unwrap().push((enabled, actor.person));
                }
                if installed_enable {
                    *self.installed_request.lock().unwrap() = request_id.clone();
                }
                self.commands.lock().unwrap().insert(
                    request_id,
                    PreferenceCommandReceipt {
                        organization_id,
                        principal_id,
                        surface_id,
                        enabled,
                        outcome: PreferenceCommandOutcome::Stored(enabled),
                    },
                );
                (PreferenceUpdate::Stored(enabled), installed_enable)
            };
            let revision = self.revision.load(Ordering::SeqCst);
            Box::pin(async move {
                Ok(RecordedPreference {
                    update,
                    revision,
                    installed_enable,
                    replayed: false,
                })
            })
        }
        fn settle_installed_preference(&self, revision: u64) -> ConversationFuture<'_, bool> {
            let settled = self.settle_revision(revision);
            Box::pin(async move { Ok(settled) })
        }
        fn revert_installed_preference(
            &self,
            revision: u64,
            actor: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            let matches = self.revision.load(Ordering::SeqCst) == revision
                && self.enabled.load(Ordering::SeqCst)
                && !self.settled.load(Ordering::SeqCst);
            if matches {
                self.enabled.store(false, Ordering::SeqCst);
                self.revision.fetch_add(1, Ordering::SeqCst);
                self.settled.store(true, Ordering::SeqCst);
                self.changes.lock().unwrap().push((false, actor.person));
                let request = std::mem::take(&mut *self.installed_request.lock().unwrap());
                let saved = self.commands.lock().unwrap().get(&request).cloned();
                if let Some(saved) = saved {
                    self.commands.lock().unwrap().insert(
                        request,
                        PreferenceCommandReceipt {
                            outcome: PreferenceCommandOutcome::Failed,
                            ..saved
                        },
                    );
                }
            }
            Box::pin(async move { Ok(matches) })
        }
        fn fail_preference_command(&self, request_id: &str) -> ConversationFuture<'_, ()> {
            let request_id = request_id.to_owned();
            let saved = self.commands.lock().unwrap().get(&request_id).cloned();
            if let Some(saved) = saved {
                if matches!(saved.outcome, PreferenceCommandOutcome::Stored(_)) {
                    self.commands.lock().unwrap().insert(
                        request_id,
                        PreferenceCommandReceipt {
                            outcome: PreferenceCommandOutcome::Failed,
                            ..saved
                        },
                    );
                }
            }
            Box::pin(async { Ok(()) })
        }
        fn status(&self, _: &str) -> ConversationFuture<'_, Option<String>> {
            refused()
        }
        fn record_outcome(
            &self,
            execution_id: &str,
            _: ScheduleSettlement,
            _: &str,
            at_ms: u64,
        ) -> ConversationFuture<'_, ()> {
            self.outcomes
                .lock()
                .unwrap()
                .push((execution_id.to_owned(), at_ms));
            Box::pin(async { Ok(()) })
        }
        fn erase_conversation(&self, _: &str, _: u64) -> ConversationFuture<'_, ()> {
            refused()
        }
        fn defer_submit(
            &self,
            execution_id: &str,
            failures: u32,
            retry_at_ms: u64,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            self.retry_failures.store(failures, Ordering::SeqCst);
            self.retry_at_ms.store(retry_at_ms, Ordering::SeqCst);
            self.defers
                .lock()
                .unwrap()
                .push((execution_id.to_owned(), failures, retry_at_ms));
            Box::pin(async { Ok(true) })
        }
        fn note_reconcile_retry(
            &self,
            conversation: &str,
            failures: u32,
            retry_at_ms: u64,
        ) -> ConversationFuture<'_, ()> {
            if self.fail_reconcile_note.swap(false, Ordering::SeqCst) {
                return refused();
            }
            self.reconciles
                .lock()
                .unwrap()
                .insert(conversation.to_owned(), (failures, retry_at_ms));
            Box::pin(async { Ok(()) })
        }
        fn clear_reconcile_retry(&self, conversation: &str) -> ConversationFuture<'_, ()> {
            self.reconciles.lock().unwrap().remove(conversation);
            Box::pin(async { Ok(()) })
        }
        fn reconcile_retries(&self) -> ConversationFuture<'_, Vec<super::ReconcileRetry>> {
            let rows = self
                .reconciles
                .lock()
                .unwrap()
                .iter()
                .map(
                    |(conversation, (failures, retry_at_ms))| super::ReconcileRetry {
                        conversation: conversation.clone(),
                        failures: *failures,
                        retry_at_ms: *retry_at_ms,
                    },
                )
                .collect();
            Box::pin(async move { Ok(rows) })
        }
    }

    fn bind(service: &ConversationService, store: Arc<RecordingStore>) {
        service.bind_scheduled_messages(store);
        service.bind_usage_limit_preference_policy(Arc::new(AdminOnly));
    }

    struct TwoOwners {
        seen: Mutex<Vec<String>>,
        fail: bool,
    }

    impl ConversationListing for TwoOwners {
        fn list(
            &self,
            _: &OrganizationId,
            _: &PrincipalId,
            _: bool,
            _: usize,
        ) -> ConversationFuture<'_, ListedConversations> {
            Box::pin(async { Ok(ListedConversations::default()) })
        }
        fn observe(
            &self,
            _: &OrganizationId,
            owner: &PrincipalId,
            _: bool,
            _: Option<ObservationCursor>,
        ) -> ConversationFuture<'_, ObservedConversations> {
            self.seen.lock().unwrap().push(owner.as_str().to_owned());
            let fail = self.fail;
            Box::pin(async move {
                Ok(ObservedConversations {
                    conversations: Vec::new(),
                    complete: !fail,
                    cursor: None,
                })
            })
        }
        fn owners(&self) -> ConversationFuture<'_, Vec<(OrganizationId, PrincipalId)>> {
            Box::pin(async {
                Ok(vec![
                    (
                        OrganizationId::new("org-a").unwrap(),
                        PrincipalId::new("person-a").unwrap(),
                    ),
                    (
                        OrganizationId::new("org-b").unwrap(),
                        PrincipalId::new("person-b").unwrap(),
                    ),
                ])
            })
        }
    }

    struct NotifyOnDue {
        wakeup: Arc<tokio::sync::Notify>,
        scans: Arc<AtomicUsize>,
    }

    impl ScheduledMessageStore for NotifyOnDue {
        fn preference(&self) -> ConversationFuture<'_, PreferenceReading> {
            Box::pin(async {
                Ok(PreferenceReading {
                    enabled: true,
                    revision: 0,
                })
            })
        }
        fn set_preference(
            &self,
            _: bool,
            _: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn open(&self, _: &str) -> ConversationFuture<'_, Option<super::ScheduledMessage>> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn accept(
            &self,
            _: ScheduleDraft,
            _: u64,
        ) -> ConversationFuture<'_, Option<super::ScheduledMessage>> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn move_due(&self, _: &str, _: &str, _: u64, _: u64) -> ConversationFuture<'_, bool> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn defer_claim(&self, _: &str, _: &str, _: u64, _: u64) -> ConversationFuture<'_, bool> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn cancel_open(
            &self,
            _: &str,
            _: ContinuationCause,
            _: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn claim_due(
            &self,
            _: u64,
            _: usize,
        ) -> ConversationFuture<'_, Vec<super::ScheduledMessage>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn next_due(&self, _: u64) -> ConversationFuture<'_, Option<u64>> {
            // One accept, during the first read. A later read must not notify:
            // the loop would otherwise stay ready and never park.
            let scan = self.scans.fetch_add(1, Ordering::SeqCst);
            if scan == 0 {
                self.wakeup.notify_waiters();
            }
            Box::pin(async { Ok(None) })
        }
        fn begin_submit(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn renew_fence(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
            Box::pin(async { Ok(false) })
        }
        fn release_submit(&self, _: &str) -> ConversationFuture<'_, ()> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn retire_unfenced_submissions(&self, _: u64) -> ConversationFuture<'_, usize> {
            Box::pin(async { Ok(0) })
        }
        fn next_live_fence(&self, _: u64) -> ConversationFuture<'_, Option<u64>> {
            Box::pin(async { Ok(None) })
        }
        fn record_preference_decision(
            &self,
            _: bool,
            _: bool,
            _: &[String],
            _: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, RecordedPreference> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn settle_installed_preference(&self, _: u64) -> ConversationFuture<'_, bool> {
            Box::pin(async { Ok(false) })
        }
        fn revert_installed_preference(
            &self,
            _: u64,
            _: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            Box::pin(async { Ok(false) })
        }
        fn status(&self, _: &str) -> ConversationFuture<'_, Option<String>> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn record_outcome(
            &self,
            _: &str,
            _: ScheduleSettlement,
            _: &str,
            _: u64,
        ) -> ConversationFuture<'_, ()> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn erase_conversation(&self, _: &str, _: u64) -> ConversationFuture<'_, ()> {
            Box::pin(async { Err(ConversationError::Unavailable) })
        }
        fn defer_submit(&self, _: &str, _: u32, _: u64, _: u64) -> ConversationFuture<'_, bool> {
            Box::pin(async { Ok(false) })
        }
        fn note_reconcile_retry(&self, _: &str, _: u32, _: u64) -> ConversationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn clear_reconcile_retry(&self, _: &str) -> ConversationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn reconcile_retries(&self) -> ConversationFuture<'_, Vec<super::ReconcileRetry>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    struct ShiftClock(AtomicU64);

    impl Clock for ShiftClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct ShiftStorage {
        clock: Arc<ShiftClock>,
    }

    struct ShiftLease;

    impl CreationStorage for ShiftStorage {
        fn open_creation(
            &self,
            _: String,
            _: String,
            _: bool,
        ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
            self.clock.0.store(9_000, Ordering::SeqCst);
            Box::pin(async { Ok(Some(Arc::new(ShiftLease) as Arc<dyn CreationStorageLease>)) })
        }
    }

    impl CreationStorageLease for ShiftLease {
        fn load(&self, _: &str) -> CreationFuture<'_, Option<CreationReceipt>> {
            Box::pin(async { Ok(None) })
        }
        fn save(&self, _: CreationReceipt) -> CreationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn load_mutation(&self, _: &str) -> CreationFuture<'_, Option<MutationReceipt>> {
            Box::pin(async { Ok(None) })
        }
        fn save_mutation(&self, _: MutationReceipt) -> CreationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn enabling_the_preference_scans_every_catalogue_owner() {
        let listing = Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        });
        let service = service(listing.clone(), Arc::new(TestClock));
        let store = Arc::new(RecordingStore::new(false));
        bind(&service, store.clone());
        let update = service
            .set_usage_limit_auto_continue(true, caller(), facts("admin"))
            .await
            .unwrap();
        assert_eq!(update, PreferenceUpdate::Stored(true));
        assert_eq!(
            listing.seen.lock().unwrap().as_slice(),
            ["person-a", "person-b"]
        );
        assert_eq!(
            store.decisions.lock().unwrap().as_slice(),
            [(true, vec!["admin".into()])]
        );
    }

    #[tokio::test]
    async fn a_failed_catalogue_scan_restores_an_off_preference() {
        let listing = Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: true,
        });
        let service = service(listing, Arc::new(TestClock));
        let store = Arc::new(RecordingStore::new(false));
        bind(&service, store.clone());
        let error = service
            .set_usage_limit_auto_continue(true, caller(), facts("admin"))
            .await;
        assert!(error.is_err());
        assert!(!store.preference().await.unwrap().enabled);
        assert_eq!(
            store.changes.lock().unwrap().as_slice(),
            &[(true, true), (false, false)]
        );
    }

    #[tokio::test]
    async fn a_failed_scan_leaves_a_preference_that_was_already_on() {
        let listing = Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: true,
        });
        let service = service(listing, Arc::new(TestClock));
        let store = Arc::new(RecordingStore::new(false));
        store
            .set_preference(true, ScheduleActor::system(), 1)
            .await
            .unwrap();
        bind(&service, store.clone());
        let error = service
            .set_usage_limit_auto_continue(true, caller(), facts("admin"))
            .await;
        assert!(error.is_err());
        assert!(store.preference().await.unwrap().enabled);
        assert_eq!(store.changes.lock().unwrap().as_slice(), &[(true, false)]);
        let receipt = store.preference_command("toggle").await.unwrap().unwrap();
        assert!(matches!(receipt.outcome, PreferenceCommandOutcome::Failed));
        let retry = service
            .set_usage_limit_auto_continue(true, caller(), facts("admin"))
            .await;
        assert!(matches!(retry, Err(ConversationError::Unavailable)));
        assert!(store.preference().await.unwrap().enabled);
        assert_eq!(store.changes.lock().unwrap().as_slice(), &[(true, false)]);
    }

    #[tokio::test]
    async fn a_lost_revision_settlement_fails_the_request_receipt() {
        let listing = Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        });
        let service = service(listing, Arc::new(TestClock));
        let store = Arc::new(RecordingStore::new(false));
        store.lose_settle.store(true, Ordering::SeqCst);
        bind(&service, store.clone());
        let error = service
            .set_usage_limit_auto_continue(true, caller(), facts("admin"))
            .await;
        assert!(matches!(error, Err(ConversationError::Unavailable)));
        assert!(store.preference().await.unwrap().enabled);
        let receipt = store.preference_command("toggle").await.unwrap().unwrap();
        assert!(matches!(receipt.outcome, PreferenceCommandOutcome::Failed));
        let retry = service
            .set_usage_limit_auto_continue(true, caller(), facts("admin"))
            .await;
        assert!(matches!(retry, Err(ConversationError::Unavailable)));
        assert_eq!(store.changes.lock().unwrap().as_slice(), &[(true, true)]);
    }

    #[tokio::test]
    async fn an_active_member_cannot_change_the_preference() {
        let listing = Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        });
        let service = service(listing.clone(), Arc::new(TestClock));
        let store = Arc::new(RecordingStore::new(false));
        bind(&service, store.clone());
        let update = service
            .set_usage_limit_auto_continue(true, caller(), facts("member"))
            .await
            .unwrap();
        assert!(matches!(update, PreferenceUpdate::Refused { .. }));
        assert!(!store.preference().await.unwrap().enabled);
        assert!(store.changes.lock().unwrap().is_empty());
        assert!(listing.seen.lock().unwrap().is_empty());
        assert_eq!(
            store.decisions.lock().unwrap().as_slice(),
            [(false, vec!["member".into()])]
        );
    }

    #[tokio::test]
    async fn a_continuation_accepted_during_the_scan_wakes_the_loop() {
        let service = service(
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
        );
        let scans = Arc::new(AtomicUsize::new(0));
        service.bind_scheduled_messages(Arc::new(NotifyOnDue {
            wakeup: service.inner.schedule_wakeup.clone(),
            scans: scans.clone(),
        }));
        let retired = service.inner.retired.clone();
        let task = tokio::spawn(async move {
            service.run_scheduled_messages().await;
        });
        let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while scans.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        retired.send_replace(true);
        let finished = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
        assert!(
            observed.is_ok(),
            "a continuation accepted during the scan did not wake the loop"
        );
        finished.expect("the schedule loop did not stop").unwrap();
    }

    #[tokio::test]
    async fn the_outcome_clock_is_read_after_submission() {
        let clock = Arc::new(ShiftClock(AtomicU64::new(1_000)));
        let service = service(
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            clock.clone(),
        );
        let store = Arc::new(RecordingStore::new(false));
        service.bind_scheduled_messages(store.clone());
        service.bind_commands(Arc::new(ShiftStorage { clock }));
        let conversation = "11111111-1111-4111-8111-111111111111";
        let row = ScheduledMessage {
            conversation: conversation.into(),
            causal_execution: "causal".into(),
            execution_id: "exec-1".into(),
            due_at_ms: 10,
            delivering: true,
            agent: "claude".into(),
            model: "opus".into(),
            prompt: "continue".into(),
            organization_id: "org".into(),
            principal_id: "person".into(),
            surface_id: "desktop".into(),
            action_id: "action".into(),
            initiation: "owner".into(),
            grant_id: None,
            policy_revision: None,
            tools_checked: false,
            retry_failures: 0,
        };
        service
            .finish_fenced_submit(
                store.clone(),
                ConversationId::new(conversation).unwrap(),
                &row,
                u64::MAX,
            )
            .await;
        assert_eq!(
            store.outcomes.lock().unwrap().as_slice(),
            [("exec-1".into(), 9_000)]
        );
    }

    /// A committed-session read that fails. Absence is `Ok(None)`.
    struct UnreadableSessions;

    impl SessionStorage for UnreadableSessions {
        fn read_committed(
            &self,
            _: SessionId,
        ) -> StorageFuture<
            '_,
            Option<nessa_sdk::application::agent_execution::sessions::CommittedSession>,
        > {
            Box::pin(async {
                Err(StorageError::Io(
                    "the committed session could not be read".into(),
                ))
            })
        }

        fn open(
            &self,
            _: SessionId,
        ) -> StorageFuture<
            '_,
            Box<dyn nessa_sdk::application::agent_execution::sessions::SessionStorageLease>,
        > {
            Box::pin(async {
                Err(StorageError::Io(
                    "the committed session could not be read".into(),
                ))
            })
        }
    }

    #[tokio::test]
    async fn a_failed_execution_read_does_not_settle_the_submission() {
        let service = service_stored(
            Arc::new(MemoryRepository::default()),
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
            Arc::new(UnreadableSessions),
        );
        let store = Arc::new(RecordingStore::new(false));
        service.bind_scheduled_messages(store.clone());
        let conversation = "11111111-1111-4111-8111-111111111111";
        let row = ScheduledMessage {
            conversation: conversation.into(),
            causal_execution: "causal".into(),
            execution_id: "exec-1".into(),
            due_at_ms: 10,
            delivering: true,
            agent: "claude".into(),
            model: "opus".into(),
            prompt: "continue".into(),
            organization_id: "org".into(),
            principal_id: "person".into(),
            surface_id: "desktop".into(),
            action_id: "action".into(),
            initiation: "owner".into(),
            grant_id: None,
            policy_revision: None,
            tools_checked: false,
            retry_failures: 0,
        };
        let retry = service
            .finish_fenced_submit(
                store.clone(),
                ConversationId::new(conversation).unwrap(),
                &row,
                1_700_000_000_123 + 60_000,
            )
            .await;
        assert!(retry);
        assert!(store.outcomes.lock().unwrap().is_empty());
        assert!(store.defers.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn disabling_the_preference_publishes_every_followed_conversation() {
        let service = service(
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
        );
        let store = Arc::new(RecordingStore::new(true));
        bind(&service, store.clone());
        let first = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
        let second = ConversationId::new("22222222-2222-4222-8222-222222222222").unwrap();
        let followed_first = service.inner.live_changes.subscribe(&first);
        let followed_second = service.inner.live_changes.subscribe(&second);
        let update = service
            .set_usage_limit_auto_continue(false, caller(), facts("admin"))
            .await
            .unwrap();
        assert_eq!(update, PreferenceUpdate::Stored(false));
        assert!(followed_first.has_changed().unwrap());
        assert!(followed_second.has_changed().unwrap());
    }

    #[tokio::test]
    async fn a_denied_preference_does_not_publish() {
        let service = service(
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
        );
        let store = Arc::new(RecordingStore::new(false));
        bind(&service, store.clone());
        let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
        let followed = service.inner.live_changes.subscribe(&id);
        let update = service
            .set_usage_limit_auto_continue(true, caller(), facts("member"))
            .await
            .unwrap();
        assert!(matches!(update, PreferenceUpdate::Refused { .. }));
        assert!(!followed.has_changed().unwrap());
    }

    #[tokio::test]
    async fn a_retried_enable_after_a_later_disable_does_not_turn_the_preference_back_on() {
        let listing = Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        });
        let service = service(listing.clone(), Arc::new(TestClock));
        let store = Arc::new(RecordingStore::new(false));
        bind(&service, store.clone());
        let enabled = service
            .set_usage_limit_auto_continue(true, caller_as("enable"), facts("admin"))
            .await
            .unwrap();
        assert_eq!(enabled, PreferenceUpdate::Stored(true));
        let seen = listing.seen.lock().unwrap().clone();
        assert_eq!(seen.as_slice(), ["person-a", "person-b"]);
        let disabled = service
            .set_usage_limit_auto_continue(false, caller_as("disable"), facts("admin"))
            .await
            .unwrap();
        assert_eq!(disabled, PreferenceUpdate::Stored(false));
        assert!(!store.preference().await.unwrap().enabled);
        let decisions = store.decisions.lock().unwrap().len();
        let replay = service
            .set_usage_limit_auto_continue(true, caller_as("enable"), facts("admin"))
            .await
            .unwrap();
        assert_eq!(replay, PreferenceUpdate::Stored(true));
        assert!(!store.preference().await.unwrap().enabled);
        assert_eq!(store.decisions.lock().unwrap().len(), decisions);
        assert_eq!(listing.seen.lock().unwrap().as_slice(), seen.as_slice());
        assert_eq!(
            store.changes.lock().unwrap().as_slice(),
            &[(true, true), (false, true)]
        );
    }

    #[tokio::test]
    async fn a_preference_request_with_a_different_value_conflicts_and_writes_nothing() {
        let service = service(
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
        );
        let store = Arc::new(RecordingStore::new(false));
        bind(&service, store.clone());
        service
            .set_usage_limit_auto_continue(true, caller_as("one"), facts("admin"))
            .await
            .unwrap();
        let conflict = service
            .set_usage_limit_auto_continue(false, caller_as("one"), facts("admin"))
            .await;
        assert!(matches!(conflict, Err(ConversationError::RequestConflict)));
        assert!(store.preference().await.unwrap().enabled);
        assert_eq!(store.decisions.lock().unwrap().len(), 1);
        let mut other = caller_as("one");
        other.principal_id = PrincipalId::new("other").unwrap();
        let mut other_facts = facts("admin");
        other_facts.principal_id = "other".into();
        let conflict = service
            .set_usage_limit_auto_continue(true, other, other_facts)
            .await;
        assert!(matches!(conflict, Err(ConversationError::RequestConflict)));
        assert!(store.preference().await.unwrap().enabled);
        assert_eq!(store.decisions.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_blank_or_unreadable_preference_request_is_rejected_before_the_decision() {
        let evaluations = Arc::new(AtomicUsize::new(0));
        struct Counting {
            evaluations: Arc<AtomicUsize>,
        }
        impl UsageLimitPreferencePolicyEvaluator for Counting {
            fn evaluate(
                &self,
                _: &UsageLimitPreferenceFacts,
            ) -> Result<UsageLimitPreferenceDecision, AccessError> {
                self.evaluations.fetch_add(1, Ordering::SeqCst);
                Ok(UsageLimitPreferenceDecision::Allow {
                    policies: vec!["admin".into()],
                })
            }
        }
        let service = service(
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
        );
        let store = Arc::new(RecordingStore::new(false));
        service.bind_scheduled_messages(store.clone());
        service.bind_usage_limit_preference_policy(Arc::new(Counting {
            evaluations: evaluations.clone(),
        }));
        let oversized = "x".repeat(257);
        for action in ["", " ", "toggle\n", "toggle\u{0001}", oversized.as_str()] {
            let error = service
                .set_usage_limit_auto_continue(true, caller_as(action), facts("admin"))
                .await;
            assert!(
                matches!(error, Err(ConversationError::InvalidInput)),
                "{action:?} {error:?}"
            );
        }
        assert_eq!(evaluations.load(Ordering::SeqCst), 0);
        assert!(store.decisions.lock().unwrap().is_empty());
        assert!(store.commands.lock().unwrap().is_empty());
        assert!(!store.preference().await.unwrap().enabled);
    }

    #[tokio::test]
    async fn a_reverted_enable_retries_as_the_same_failure() {
        let listing = Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: true,
        });
        let service = service(listing.clone(), Arc::new(TestClock));
        let store = Arc::new(RecordingStore::new(false));
        bind(&service, store.clone());
        let error = service
            .set_usage_limit_auto_continue(true, caller_as("enable"), facts("admin"))
            .await;
        assert!(error.is_err());
        assert!(!store.preference().await.unwrap().enabled);
        let seen = listing.seen.lock().unwrap().len();
        let decisions = store.decisions.lock().unwrap().len();
        let retry = service
            .set_usage_limit_auto_continue(true, caller_as("enable"), facts("admin"))
            .await;
        assert!(matches!(retry, Err(ConversationError::Unavailable)));
        assert!(!store.preference().await.unwrap().enabled);
        assert_eq!(listing.seen.lock().unwrap().len(), seen);
        assert_eq!(store.decisions.lock().unwrap().len(), decisions);
    }

    #[tokio::test]
    async fn a_failed_preference_audit_leaves_the_preference_and_returns_an_error() {
        let service = service(
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
        );
        let store = Arc::new(RecordingStore::new(false));
        bind(&service, store.clone());
        store.fail_decision.store(true, Ordering::SeqCst);
        let error = service
            .set_usage_limit_auto_continue(true, caller(), facts("admin"))
            .await;
        assert!(error.is_err());
        assert!(!store.preference().await.unwrap().enabled);
        assert!(store.changes.lock().unwrap().is_empty());
        assert!(store.decisions.lock().unwrap().is_empty());

        store.fail_decision.store(true, Ordering::SeqCst);
        let denied = service
            .set_usage_limit_auto_continue(true, caller(), facts("member"))
            .await;
        assert!(denied.is_err());
        assert!(!store.preference().await.unwrap().enabled);
        assert!(store.decisions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failed_schedule_plan_does_not_submit() {
        let repository = Arc::new(MemoryRepository::default());
        let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
        repository.records.lock().unwrap().insert(
            id.clone(),
            crate::conversation::domain::Conversation::restore(
                id.clone(),
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("person").unwrap(),
                "desktop".into(),
                "create".into(),
                1,
                None,
                ConversationModelId::new("opus").unwrap(),
                ConversationApprovalMode::Ask,
            )
            .unwrap(),
        );
        let service = service_over(
            repository,
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
        );
        let store = Arc::new(RecordingStore::new(true));
        service.bind_scheduled_messages(store.clone());
        let row = ScheduledMessage {
            conversation: id.to_string(),
            causal_execution: "causal".into(),
            execution_id: "exec-1".into(),
            due_at_ms: 10,
            delivering: true,
            agent: "claude".into(),
            model: "opus".into(),
            prompt: "continue".into(),
            organization_id: "org".into(),
            principal_id: "person".into(),
            surface_id: "desktop".into(),
            action_id: "action".into(),
            initiation: "owner".into(),
            grant_id: None,
            policy_revision: None,
            tools_checked: false,
            retry_failures: 0,
        };
        service.deliver_scheduled(store.clone(), row).await;
        assert_eq!(store.opens.load(Ordering::SeqCst), 1);
        assert_eq!(store.submits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_applied_model_change_is_not_complete_until_reconciliation_commits() {
        let repository = Arc::new(MemoryRepository::default());
        let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
        let organization = OrganizationId::new("org").unwrap();
        let principal = PrincipalId::new("person").unwrap();
        let prior_model = ConversationModelId::new("opus").unwrap();
        let requested = ConversationModelId::new("sonnet").unwrap();
        repository.records.lock().unwrap().insert(
            id.clone(),
            crate::conversation::domain::Conversation::restore(
                id.clone(),
                organization.clone(),
                principal.clone(),
                "desktop".into(),
                "create".into(),
                1,
                None,
                prior_model.clone(),
                ConversationApprovalMode::Ask,
            )
            .unwrap(),
        );
        let selection = ConversationModelSelection {
            model: requested.clone(),
            approval_mode: ConversationApprovalMode::Ask,
            effect: ConversationModelChangeEffect::NextTurn,
        };
        repository.model_requests.lock().unwrap().insert(
            (id.clone(), "toggle".into()),
            ConversationModelRequest {
                conversation_id: id.clone(),
                organization_id: organization,
                request_id: "toggle".into(),
                initiator_principal_id: principal.clone(),
                initiator_surface_id: "desktop".into(),
                prior_identity: ProviderIdentity::new("fixture", "opus", "").unwrap(),
                requested_identity: ProviderIdentity::new("fixture", "sonnet", "").unwrap(),
                prior_model,
                prior_approval_mode: ConversationApprovalMode::Ask,
                requested_model: requested.clone(),
                state: ConversationModelRequestState::Applied,
                application: Some(ConversationModelApplication::Applied(selection.clone())),
                requested_at_ms: 1,
            },
        );
        let service = service_over(
            repository.clone(),
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
        );
        let store = Arc::new(RecordingStore::new(false));
        service.bind_scheduled_messages(store.clone());
        let caller = caller();
        let failed = service.change_model(&id, &caller, requested.clone()).await;
        assert!(failed.is_err());
        assert_eq!(store.opens.load(Ordering::SeqCst), 1);
        store.fail_open.store(false, Ordering::SeqCst);
        let applied = service.change_model(&id, &caller, requested).await.unwrap();
        assert_eq!(applied, selection);
        assert_eq!(store.opens.load(Ordering::SeqCst), 2);

        let applied_request = repository
            .model_requests
            .lock()
            .unwrap()
            .get(&(id.clone(), "toggle".into()))
            .unwrap()
            .clone();
        repository.model_requests.lock().unwrap().insert(
            (id.clone(), "refused".into()),
            ConversationModelRequest {
                request_id: "refused".into(),
                state: ConversationModelRequestState::NotApplied,
                application: Some(ConversationModelApplication::Refused),
                ..applied_request
            },
        );
        let opens = store.opens.load(Ordering::SeqCst);
        let mut refused_caller = caller;
        refused_caller.action_id = "refused".into();
        let refused = service
            .change_model(
                &id,
                &refused_caller,
                ConversationModelId::new("sonnet").unwrap(),
            )
            .await;
        assert!(matches!(
            refused,
            Err(ConversationError::ModelChangeNotApplied)
        ));
        assert_eq!(store.opens.load(Ordering::SeqCst), opens);
    }

    #[tokio::test]
    async fn a_failed_model_reconciliation_is_retried_when_nothing_is_pending() {
        let repository = Arc::new(MemoryRepository::default());
        let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
        let organization = OrganizationId::new("org").unwrap();
        let principal = PrincipalId::new("person").unwrap();
        let prior_model = ConversationModelId::new("opus").unwrap();
        let requested = ConversationModelId::new("sonnet").unwrap();
        repository.records.lock().unwrap().insert(
            id.clone(),
            crate::conversation::domain::Conversation::restore(
                id.clone(),
                organization.clone(),
                principal.clone(),
                "desktop".into(),
                "create".into(),
                1,
                None,
                prior_model.clone(),
                ConversationApprovalMode::Ask,
            )
            .unwrap(),
        );
        repository.model_requests.lock().unwrap().insert(
            (id.clone(), "toggle".into()),
            ConversationModelRequest {
                conversation_id: id.clone(),
                organization_id: organization,
                request_id: "toggle".into(),
                initiator_principal_id: principal,
                initiator_surface_id: "desktop".into(),
                prior_identity: ProviderIdentity::new("fixture", "opus", "").unwrap(),
                requested_identity: ProviderIdentity::new("fixture", "sonnet", "").unwrap(),
                prior_model,
                prior_approval_mode: ConversationApprovalMode::Ask,
                requested_model: requested.clone(),
                state: ConversationModelRequestState::Applied,
                application: Some(ConversationModelApplication::Applied(
                    ConversationModelSelection {
                        model: requested.clone(),
                        approval_mode: ConversationApprovalMode::Ask,
                        effect: ConversationModelChangeEffect::NextTurn,
                    },
                )),
                requested_at_ms: 1,
            },
        );
        let service = service_over(
            repository,
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            Arc::new(TestClock),
        );
        let store = Arc::new(RecordingStore::new(false));
        service.bind_scheduled_messages(store.clone());
        let caller = caller();
        assert!(service.change_model(&id, &caller, requested).await.is_err());
        assert_eq!(store.opens.load(Ordering::SeqCst), 1);
        assert!(service.recover_model_change(&id, &caller).await.is_err());
        assert_eq!(store.opens.load(Ordering::SeqCst), 2);
        store.fail_open.store(false, Ordering::SeqCst);
        service.recover_model_change(&id, &caller).await.unwrap();
        assert_eq!(store.opens.load(Ordering::SeqCst), 3);
        let opens = store.opens.load(Ordering::SeqCst);
        service.recover_model_change(&id, &caller).await.unwrap();
        assert_eq!(store.opens.load(Ordering::SeqCst), opens);
    }

    /// A catalogue page that settles the enable, then fails the walk.
    struct SettleThenFail {
        store: Arc<RecordingStore>,
    }

    impl ConversationListing for SettleThenFail {
        fn list(
            &self,
            _: &OrganizationId,
            _: &PrincipalId,
            _: bool,
            _: usize,
        ) -> ConversationFuture<'_, ListedConversations> {
            Box::pin(async { Ok(ListedConversations::default()) })
        }
        fn observe(
            &self,
            _: &OrganizationId,
            _: &PrincipalId,
            _: bool,
            _: Option<ObservationCursor>,
        ) -> ConversationFuture<'_, ObservedConversations> {
            let revision = self.store.revision.load(Ordering::SeqCst);
            self.store.settle_revision(revision);
            Box::pin(async {
                Ok(ObservedConversations {
                    conversations: Vec::new(),
                    complete: false,
                    cursor: None,
                })
            })
        }
        fn owners(&self) -> ConversationFuture<'_, Vec<(OrganizationId, PrincipalId)>> {
            Box::pin(async {
                Ok(vec![(
                    OrganizationId::new("org-a").unwrap(),
                    PrincipalId::new("person-a").unwrap(),
                )])
            })
        }
    }

    #[tokio::test]
    async fn a_settled_enable_is_not_rolled_back_by_a_later_scan_failure() {
        let store = Arc::new(RecordingStore::new(false));
        let service = service(
            Arc::new(SettleThenFail {
                store: store.clone(),
            }),
            Arc::new(TestClock),
        );
        bind(&service, store.clone());
        let error = service
            .set_usage_limit_auto_continue(true, caller(), facts("admin"))
            .await;
        assert!(error.is_err());
        assert!(store.preference().await.unwrap().enabled);
        assert_eq!(store.changes.lock().unwrap().as_slice(), &[(true, true)]);
    }

    /// Schedule-store reads that fail a counted number of times, and a scripted renew.
    struct ScriptedSchedule {
        enabled: AtomicBool,
        preference_failures: AtomicUsize,
        claim_failures: AtomicUsize,
        next_due_failures: AtomicUsize,
        fence_failures: AtomicUsize,
        retire_failures: AtomicUsize,
        renew: Mutex<Vec<Result<bool, ConversationError>>>,
        renew_calls: AtomicUsize,
    }

    impl ScriptedSchedule {
        fn new(enabled: bool) -> Self {
            Self {
                enabled: AtomicBool::new(enabled),
                preference_failures: AtomicUsize::new(0),
                claim_failures: AtomicUsize::new(0),
                next_due_failures: AtomicUsize::new(0),
                fence_failures: AtomicUsize::new(0),
                retire_failures: AtomicUsize::new(0),
                renew: Mutex::new(Vec::new()),
                renew_calls: AtomicUsize::new(0),
            }
        }
    }

    fn failing(counter: &AtomicUsize) -> bool {
        loop {
            let left = counter.load(Ordering::SeqCst);
            if left == 0 {
                return false;
            }
            if counter
                .compare_exchange(left, left - 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }

    fn store_error() -> ConversationFuture<'static, bool> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }

    impl ScheduledMessageStore for ScriptedSchedule {
        fn preference(&self) -> ConversationFuture<'_, PreferenceReading> {
            if failing(&self.preference_failures) {
                return Box::pin(async { Err(ConversationError::Metadata) });
            }
            let enabled = self.enabled.load(Ordering::SeqCst);
            Box::pin(async move {
                Ok(PreferenceReading {
                    enabled,
                    revision: 0,
                })
            })
        }
        fn set_preference(
            &self,
            _: bool,
            _: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            store_error()
        }
        fn open(&self, _: &str) -> ConversationFuture<'_, Option<ScheduledMessage>> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn accept(
            &self,
            _: ScheduleDraft,
            _: u64,
        ) -> ConversationFuture<'_, Option<ScheduledMessage>> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn move_due(&self, _: &str, _: &str, _: u64, _: u64) -> ConversationFuture<'_, bool> {
            store_error()
        }
        fn defer_claim(&self, _: &str, _: &str, _: u64, _: u64) -> ConversationFuture<'_, bool> {
            store_error()
        }
        fn cancel_open(
            &self,
            _: &str,
            _: ContinuationCause,
            _: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            store_error()
        }
        fn claim_due(&self, _: u64, _: usize) -> ConversationFuture<'_, Vec<ScheduledMessage>> {
            if failing(&self.claim_failures) {
                return Box::pin(async { Err(ConversationError::Metadata) });
            }
            Box::pin(async { Ok(Vec::new()) })
        }
        fn next_due(&self, _: u64) -> ConversationFuture<'_, Option<u64>> {
            if failing(&self.next_due_failures) {
                return Box::pin(async { Err(ConversationError::Metadata) });
            }
            Box::pin(async { Ok(None) })
        }
        fn begin_submit(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
            store_error()
        }
        fn renew_fence(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
            self.renew_calls.fetch_add(1, Ordering::SeqCst);
            let next = {
                let mut renew = self.renew.lock().unwrap();
                if renew.is_empty() {
                    None
                } else {
                    Some(renew.remove(0))
                }
            };
            Box::pin(async move {
                match next {
                    Some(result) => result,
                    None => Ok(true),
                }
            })
        }
        fn release_submit(&self, _: &str) -> ConversationFuture<'_, ()> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn retire_unfenced_submissions(&self, _: u64) -> ConversationFuture<'_, usize> {
            if failing(&self.retire_failures) {
                return Box::pin(async { Err(ConversationError::Metadata) });
            }
            Box::pin(async { Ok(0) })
        }
        fn next_live_fence(&self, _: u64) -> ConversationFuture<'_, Option<u64>> {
            if failing(&self.fence_failures) {
                return Box::pin(async { Err(ConversationError::Metadata) });
            }
            Box::pin(async { Ok(None) })
        }
        fn record_preference_decision(
            &self,
            _: bool,
            _: bool,
            _: &[String],
            _: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, RecordedPreference> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn settle_installed_preference(&self, _: u64) -> ConversationFuture<'_, bool> {
            store_error()
        }
        fn revert_installed_preference(
            &self,
            _: u64,
            _: ScheduleActor,
            _: u64,
        ) -> ConversationFuture<'_, bool> {
            store_error()
        }
        fn status(&self, _: &str) -> ConversationFuture<'_, Option<String>> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn record_outcome(
            &self,
            _: &str,
            _: ScheduleSettlement,
            _: &str,
            _: u64,
        ) -> ConversationFuture<'_, ()> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn erase_conversation(&self, _: &str, _: u64) -> ConversationFuture<'_, ()> {
            Box::pin(async { Err(ConversationError::Metadata) })
        }
        fn defer_submit(&self, _: &str, _: u32, _: u64, _: u64) -> ConversationFuture<'_, bool> {
            Box::pin(async { Ok(false) })
        }
        fn note_reconcile_retry(&self, _: &str, _: u32, _: u64) -> ConversationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn clear_reconcile_retry(&self, _: &str) -> ConversationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn reconcile_retries(&self) -> ConversationFuture<'_, Vec<super::ReconcileRetry>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[tokio::test]
    async fn a_store_error_retries_delivery_instead_of_parking() {
        let listing = Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        });
        let unbound = service(listing.clone(), Arc::new(TestClock));
        assert_eq!(unbound.deliver_due_scheduled().await, None);

        let schedule = Arc::new(ScriptedSchedule::new(false));
        let service = service(listing, Arc::new(TestClock));
        service.bind_scheduled_messages(schedule.clone());

        schedule.preference_failures.store(1, Ordering::SeqCst);
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
        schedule.retire_failures.store(1, Ordering::SeqCst);
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
        schedule.fence_failures.store(1, Ordering::SeqCst);
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
        assert_eq!(service.deliver_due_scheduled().await, None);

        schedule.enabled.store(true, Ordering::SeqCst);
        schedule.claim_failures.store(1, Ordering::SeqCst);
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
        schedule.next_due_failures.store(1, Ordering::SeqCst);
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
        assert_eq!(service.deliver_due_scheduled().await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_fence_renewal_is_retried() {
        let schedule = Arc::new(ScriptedSchedule::new(true));
        schedule
            .renew
            .lock()
            .unwrap()
            .push(Err(ConversationError::Metadata));
        schedule.renew.lock().unwrap().push(Ok(true));
        let lost = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(renew_while_held(
            schedule.clone(),
            "exec-1".into(),
            Arc::new(TestClock),
            lost.clone(),
            TestClock.unix_milliseconds() + FENCE_LEASE_MS,
        ));
        tokio::time::sleep(Duration::from_millis(
            FENCE_RENEW_EVERY_MS + FENCE_RENEW_RETRY_MS + 1,
        ))
        .await;
        assert!(schedule.renew_calls.load(Ordering::SeqCst) >= 2);
        assert!(!lost.load(Ordering::SeqCst));
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_lost_fence_stops_renewal() {
        let schedule = Arc::new(ScriptedSchedule::new(true));
        schedule.renew.lock().unwrap().push(Ok(false));
        let lost = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(renew_while_held(
            schedule.clone(),
            "exec-1".into(),
            Arc::new(TestClock),
            lost.clone(),
            TestClock.unix_milliseconds() + FENCE_LEASE_MS,
        ));
        tokio::time::sleep(Duration::from_millis(FENCE_RENEW_EVERY_MS + 1)).await;
        assert!(lost.load(Ordering::SeqCst));
        assert_eq!(schedule.renew_calls.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(FENCE_RENEW_EVERY_MS)).await;
        assert_eq!(schedule.renew_calls.load(Ordering::SeqCst), 1);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_renewal_error_after_the_lease_expires_stops() {
        let schedule = Arc::new(ScriptedSchedule::new(true));
        for _ in 0..40 {
            schedule
                .renew
                .lock()
                .unwrap()
                .push(Err(ConversationError::Metadata));
        }
        let clock = Arc::new(PausingClock {
            origin: tokio::time::Instant::now(),
            base: 1_000,
        });
        let lost = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(renew_while_held(
            schedule.clone(),
            "exec-1".into(),
            clock,
            lost.clone(),
            1_000 + FENCE_LEASE_MS,
        ));
        tokio::time::sleep(Duration::from_millis(FENCE_LEASE_MS + FENCE_RENEW_EVERY_MS)).await;
        assert!(lost.load(Ordering::SeqCst));
        let calls = schedule.renew_calls.load(Ordering::SeqCst);
        assert!(calls >= 1);
        tokio::time::sleep(Duration::from_millis(FENCE_RENEW_EVERY_MS * 3)).await;
        assert_eq!(schedule.renew_calls.load(Ordering::SeqCst), calls);
        task.abort();
    }

    #[test]
    fn a_continuation_prompt_stays_within_the_input_limit() {
        let notice =
            "The usage limit reset. Continue the request that was waiting.\n\nThe request was:\n";
        let request = "please continue the request that was waiting on the limit";
        for max in [1_usize, notice.len() - 1, notice.len(), notice.len() + 4] {
            let prompt = super::continuation_prompt(request, max);
            assert!(prompt.len() <= max, "{prompt:?} exceeds {max}");
            assert!(!prompt.trim().is_empty());
        }
        let fitted = super::continuation_prompt("hello", notice.len() + 5);
        assert!(fitted.starts_with(notice));
        assert!(fitted.ends_with("hello"));
        assert!(fitted.len() <= notice.len() + 5);
    }

    #[test]
    fn catch_up_backoff_doubles_until_it_is_capped() {
        assert_eq!(super::attempt_backoff(1), SCHEDULE_RETRY_MS);
        assert_eq!(super::attempt_backoff(2), SCHEDULE_RETRY_MS * 2);
        assert_eq!(super::attempt_backoff(3), SCHEDULE_RETRY_MS * 4);
        assert_eq!(super::attempt_backoff(30), super::CATCH_UP_RETRY_MAX_MS);
    }

    /// The first page of `person-a` cannot say where to resume. `person-b`
    /// is the owner that page skips.
    struct FailThenPlan {
        conversation: crate::conversation::domain::Conversation,
        summary: ConversationSummary,
        seen: Mutex<Vec<String>>,
        failed_pages: AtomicUsize,
    }

    impl ConversationListing for FailThenPlan {
        fn list(
            &self,
            _: &OrganizationId,
            _: &PrincipalId,
            _: bool,
            _: usize,
        ) -> ConversationFuture<'_, ListedConversations> {
            Box::pin(async { Ok(ListedConversations::default()) })
        }
        fn observe(
            &self,
            _: &OrganizationId,
            owner: &PrincipalId,
            _: bool,
            _: Option<ObservationCursor>,
        ) -> ConversationFuture<'_, ObservedConversations> {
            let name = owner.as_str().to_owned();
            self.seen.lock().unwrap().push(name.clone());
            if name == "person-a" && self.failed_pages.fetch_add(1, Ordering::SeqCst) == 0 {
                return Box::pin(async {
                    Ok(ObservedConversations {
                        conversations: Vec::new(),
                        complete: false,
                        cursor: None,
                    })
                });
            }
            let conversations = if name == "person-b" {
                vec![ListedConversation {
                    conversation: self.conversation.clone(),
                    summary: self.summary.clone(),
                }]
            } else {
                Vec::new()
            };
            Box::pin(async move {
                Ok(ObservedConversations {
                    conversations,
                    complete: true,
                    cursor: None,
                })
            })
        }
        fn owners(&self) -> ConversationFuture<'_, Vec<(OrganizationId, PrincipalId)>> {
            Box::pin(async {
                Ok(vec![
                    (
                        OrganizationId::new("org-a").unwrap(),
                        PrincipalId::new("person-a").unwrap(),
                    ),
                    (
                        OrganizationId::new("org-b").unwrap(),
                        PrincipalId::new("person-b").unwrap(),
                    ),
                ])
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_catch_up_scan_schedules_the_skipped_owner_on_a_later_pass() {
        let id = ConversationId::new("33333333-3333-4333-8333-333333333333").unwrap();
        let conversation = crate::conversation::domain::Conversation::restore(
            id.clone(),
            OrganizationId::new("org-b").unwrap(),
            PrincipalId::new("person-b").unwrap(),
            "desktop".into(),
            "create".into(),
            1,
            None,
            ConversationModelId::new("opus").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap();
        let repository = Arc::new(MemoryRepository::default());
        repository
            .records
            .lock()
            .unwrap()
            .insert(id.clone(), conversation.clone());
        let listing = Arc::new(FailThenPlan {
            conversation,
            summary: ConversationSummary::new(None, None, 1, false).unwrap(),
            seen: Mutex::new(Vec::new()),
            failed_pages: AtomicUsize::new(0),
        });
        let service = service_over(repository, listing.clone(), Arc::new(TestClock));
        let store = Arc::new(RecordingStore::new(true));
        store.fail_open.store(false, Ordering::SeqCst);
        store.idle_delivery.store(true, Ordering::SeqCst);
        service.bind_scheduled_messages(store.clone());
        let retired = service.inner.retired.clone();
        let task = tokio::spawn(async move {
            service.run_scheduled_messages().await;
        });

        let saw_failure = tokio::time::timeout(Duration::from_secs(5), async {
            while listing.failed_pages.load(Ordering::SeqCst) < 1 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(saw_failure.is_ok(), "the catalogue page did not fail");
        assert!(store.opened.lock().unwrap().is_empty());
        assert_eq!(listing.seen.lock().unwrap().as_slice(), ["person-a"]);

        tokio::time::sleep(Duration::from_millis(SCHEDULE_RETRY_MS - 1)).await;
        tokio::task::yield_now().await;
        assert_eq!(listing.failed_pages.load(Ordering::SeqCst), 1);
        assert!(store.opened.lock().unwrap().is_empty());

        tokio::time::sleep(Duration::from_millis(2)).await;
        let planned = tokio::time::timeout(Duration::from_secs(5), async {
            while !store
                .opened
                .lock()
                .unwrap()
                .iter()
                .any(|opened| opened == &id.to_string())
            {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(planned.is_ok(), "the skipped owner was not planned");
        let seen = listing.seen.lock().unwrap().clone();
        let skipped = seen
            .iter()
            .position(|owner| owner == "person-b")
            .expect("person-b was not visited");
        assert!(
            seen[..skipped]
                .iter()
                .filter(|owner| owner.as_str() == "person-a")
                .count()
                >= 2
        );

        retired.send_replace(true);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("the schedule loop did not stop")
            .unwrap();
    }

    /// A preference read that fails must not park the loop when delivery has
    /// nothing due. The next pass, after the backoff, traverses the owners.
    #[tokio::test(start_paused = true)]
    async fn a_failed_preference_read_retries_catch_up_when_nothing_is_due() {
        let id = ConversationId::new("33333333-3333-4333-8333-333333333333").unwrap();
        let conversation = crate::conversation::domain::Conversation::restore(
            id.clone(),
            OrganizationId::new("org-b").unwrap(),
            PrincipalId::new("person-b").unwrap(),
            "desktop".into(),
            "create".into(),
            1,
            None,
            ConversationModelId::new("opus").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap();
        let repository = Arc::new(MemoryRepository::default());
        repository
            .records
            .lock()
            .unwrap()
            .insert(id.clone(), conversation.clone());
        let listing = Arc::new(FailThenPlan {
            conversation,
            summary: ConversationSummary::new(None, None, 1, false).unwrap(),
            seen: Mutex::new(Vec::new()),
            failed_pages: AtomicUsize::new(1),
        });
        let service = service_over(repository, listing.clone(), Arc::new(TestClock));
        let store = Arc::new(RecordingStore::new(true));
        store.fail_open.store(false, Ordering::SeqCst);
        store.idle_delivery.store(true, Ordering::SeqCst);
        store.preference_failures.store(1, Ordering::SeqCst);
        service.bind_scheduled_messages(store.clone());
        let retired = service.inner.retired.clone();
        let task = tokio::spawn(async move {
            service.run_scheduled_messages().await;
        });

        let saw_failure = tokio::time::timeout(Duration::from_secs(5), async {
            while store.preference_failures.load(Ordering::SeqCst) > 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(saw_failure.is_ok(), "the preference read did not fail");
        assert!(store.opened.lock().unwrap().is_empty());
        assert!(listing.seen.lock().unwrap().is_empty());

        tokio::time::sleep(Duration::from_millis(SCHEDULE_RETRY_MS - 1)).await;
        tokio::task::yield_now().await;
        assert!(store.opened.lock().unwrap().is_empty());
        assert!(listing.seen.lock().unwrap().is_empty());

        tokio::time::sleep(Duration::from_millis(2)).await;
        let planned = tokio::time::timeout(Duration::from_secs(5), async {
            while !store
                .opened
                .lock()
                .unwrap()
                .iter()
                .any(|opened| opened == &id.to_string())
            {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            planned.is_ok(),
            "catch-up did not run after the preference read failed"
        );

        retired.send_replace(true);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("the schedule loop did not stop")
            .unwrap();
    }

    struct PausingClock {
        origin: tokio::time::Instant,
        base: u64,
    }

    impl Clock for PausingClock {
        fn unix_milliseconds(&self) -> u64 {
            self.base
                .saturating_add(u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(0))
        }
    }

    fn blocked_conversation(id: &ConversationId) -> crate::conversation::domain::Conversation {
        crate::conversation::domain::Conversation::restore(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "desktop".into(),
            "create".into(),
            1,
            None,
            ConversationModelId::new("opus").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap()
    }

    /// Catch-up has finished. A settlement whose plan cannot be read is tried
    /// again only after the backoff, and not on the tight loop that would
    /// otherwise see nothing due.
    #[tokio::test(start_paused = true)]
    async fn a_failed_settlement_is_retried_after_the_backoff() {
        let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
        let repository = Arc::new(MemoryRepository::default());
        repository
            .records
            .lock()
            .unwrap()
            .insert(id.clone(), blocked_conversation(&id));
        let clock = Arc::new(PausingClock {
            origin: tokio::time::Instant::now(),
            base: 5_000,
        });
        let service = service_over(
            repository,
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            clock.clone(),
        );
        let store = Arc::new(RecordingStore::new(true));
        store.idle_delivery.store(true, Ordering::SeqCst);
        service.bind_scheduled_messages(store.clone());
        service
            .consider_usage_limit(&id, false, ScheduleActor::system())
            .await;
        assert_eq!(store.opens.load(Ordering::SeqCst), 1);
        let (failures, retry_at) = store
            .reconciles
            .lock()
            .unwrap()
            .get(&id.to_string())
            .copied()
            .expect("the settlement kept a retry");
        assert_eq!(failures, 1);
        assert_eq!(
            retry_at.saturating_sub(clock.unix_milliseconds()),
            SCHEDULE_RETRY_MS
        );

        let retired = service.inner.retired.clone();
        let task = tokio::spawn(async move {
            service.run_scheduled_messages().await;
        });
        tokio::time::sleep(Duration::from_millis(SCHEDULE_RETRY_MS - 1)).await;
        tokio::task::yield_now().await;
        assert_eq!(store.opens.load(Ordering::SeqCst), 1);

        tokio::time::sleep(Duration::from_millis(2)).await;
        let retried = tokio::time::timeout(Duration::from_secs(5), async {
            while store.opens.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(retried.is_ok(), "the settlement was not retried");
        assert_eq!(store.opens.load(Ordering::SeqCst), 2);

        retired.send_replace(true);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("the schedule loop did not stop")
            .unwrap();
    }

    #[tokio::test]
    async fn a_reconcile_debt_the_database_refuses_is_still_retried() {
        let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
        let repository = Arc::new(MemoryRepository::default());
        repository
            .records
            .lock()
            .unwrap()
            .insert(id.clone(), blocked_conversation(&id));
        let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
        let service = service_over(
            repository,
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            clock.clone(),
        );
        let store = Arc::new(RecordingStore::new(true));
        store.idle_delivery.store(true, Ordering::SeqCst);
        store.fail_reconcile_note.store(true, Ordering::SeqCst);
        service.bind_scheduled_messages(store.clone());
        service
            .consider_usage_limit(&id, false, ScheduleActor::system())
            .await;
        assert!(store.reconciles.lock().unwrap().is_empty());
        assert_eq!(store.opens.load(Ordering::SeqCst), 1);
        assert!(service.retry_due_reconciles().await.unwrap() >= SCHEDULE_RETRY_MS - 1);
        assert_eq!(store.opens.load(Ordering::SeqCst), 1);
        clock
            .0
            .store(5_000 + SCHEDULE_RETRY_MS - 1, Ordering::SeqCst);
        service.retry_due_reconciles().await;
        assert_eq!(store.opens.load(Ordering::SeqCst), 1);
        clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
        service.retry_due_reconciles().await;
        assert_eq!(store.opens.load(Ordering::SeqCst), 2);
    }

    struct BusyCommands;

    impl CreationStorage for BusyCommands {
        fn open_creation(
            &self,
            _: String,
            _: String,
            _: bool,
        ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
            Box::pin(async { Err(CreationStorageError::Storage(StorageError::Busy)) })
        }
    }

    fn submitting_row(conversation: &str) -> ScheduledMessage {
        ScheduledMessage {
            conversation: conversation.into(),
            causal_execution: "causal".into(),
            execution_id: "exec-1".into(),
            due_at_ms: 10,
            delivering: true,
            agent: "claude".into(),
            model: "opus".into(),
            prompt: "continue".into(),
            organization_id: "org".into(),
            principal_id: "person".into(),
            surface_id: "desktop".into(),
            action_id: "action".into(),
            initiation: "owner".into(),
            grant_id: None,
            policy_revision: None,
            tools_checked: false,
            retry_failures: 0,
        }
    }

    /// A retryable submit failure stays retryable, and the same process does
    /// not acquire the row again before the backoff.
    #[tokio::test]
    async fn a_retryable_submit_does_not_spin() {
        let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
        let service = service(
            Arc::new(TwoOwners {
                seen: Mutex::new(Vec::new()),
                fail: false,
            }),
            clock.clone(),
        );
        let store = Arc::new(RecordingStore::new(true));
        store.submit_attempts.store(true, Ordering::SeqCst);
        let conversation = "11111111-1111-4111-8111-111111111111";
        store
            .due_row
            .lock()
            .unwrap()
            .replace(submitting_row(conversation));
        service.bind_scheduled_messages(store.clone());
        service.bind_commands(Arc::new(BusyCommands));

        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
        assert_eq!(store.submits.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.defers.lock().unwrap().as_slice(),
            [("exec-1".into(), 1, 5_000 + SCHEDULE_RETRY_MS)]
        );
        for _ in 0..30 {
            assert_eq!(
                service.deliver_due_scheduled().await,
                Some(SCHEDULE_RETRY_MS)
            );
        }
        assert_eq!(store.submits.load(Ordering::SeqCst), 1);
        assert!(store.outcomes.lock().unwrap().is_empty());

        clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS * 2)
        );
        assert_eq!(store.submits.load(Ordering::SeqCst), 2);
        assert_eq!(
            store.defers.lock().unwrap().as_slice(),
            [
                ("exec-1".into(), 1, 5_000 + SCHEDULE_RETRY_MS),
                ("exec-1".into(), 2, 5_000 + SCHEDULE_RETRY_MS * 3),
            ]
        );
    }
}
