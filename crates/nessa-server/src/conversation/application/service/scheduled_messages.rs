//! One scheduled continuation after a provider usage limit.
//!
//! The preference is one boolean for this gateway, default off. A row is the
//! durable one-shot message. Delivery calls [`ConversationService::submit_command`]
//! with the causal turn's initiation, so Cedar and a deleted conversation are
//! decided there. The compare-and-set from `delivering` to `submitting` is the
//! only permission to submit, and it takes a lease the other processes on this
//! file can see. Cancel, defer, and a model change lose to that transition and
//! win against a claim that has not made it. An applied model change is not
//! complete until that plan commits. Cedar's allow and deny of the preference
//! are each one audit before the command result.

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
    /// `draft.preference_revision` is still the enabled preference. A finished
    /// causal turn, or a revision a preference change has replaced, returns `None`.
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
    /// Claim due scheduled rows, then return those claims and rows already
    /// delivering. Each execution id appears once. The claim is not itself
    /// an audit row: the delivery or refusal audit follows admission, and a
    /// released claim is audited as `reset_moved`.
    fn claim_due(&self, now_ms: u64, limit: usize)
        -> ConversationFuture<'_, Vec<ScheduledMessage>>;
    /// When the next pass should run. `Some(now_ms)` when a scheduled row is
    /// already due, or a row is `delivering` or `submitting`, so a claim that
    /// stopped at its limit, or a delivery audit that failed, is retried on
    /// the next pass. Otherwise the soonest scheduled due time strictly after
    /// `now_ms`.
    fn next_due(&self, now_ms: u64) -> ConversationFuture<'_, Option<u64>>;
    /// Move `delivering` to `submitting` under a lease that lasts
    /// [`FENCE_LEASE_MS`] from `now_ms`, or adopt a `submitting` row whose
    /// lease has expired or that this process already owns and is not inside.
    /// True is the only permission to submit. The in-memory arm is taken only
    /// after the update commits, and only so this process does not enter twice.
    /// Cancel, defer, and a model change match `scheduled` and `delivering`
    /// only, so exactly one of those and this transition can win.
    fn begin_submit(&self, execution_id: &str, now_ms: u64) -> ConversationFuture<'_, bool>;
    /// Extend this process's lease on a `submitting` row. False when this
    /// process is not the owner, or the row is no longer `submitting`.
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
    /// Record Cedar's decision. Allow writes the preference, the audit, and
    /// any cancellations in one transaction. Deny writes only the audit.
    /// `Err` means the audit did not commit and the preference is unchanged.
    /// An unchanged allow is still audited. An allow that changes off to on
    /// installs one unsettled revision; any other allow leaves the revision
    /// where it was, or settles a disable.
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
    /// commit together. `Err` leaves the previous status. `at_ms` is the
    /// clock at the observation, not a reading taken before the submit.
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
        let Some(store) = self.scheduled_messages() else {
            return Ok(None);
        };
        if !store.preference().await?.enabled {
            return Ok(None);
        }
        store.open(&conversation.to_string()).await
    }

    /// Turn the preference on or off. Cedar decides first. Off cancels open
    /// rows as this person. On scans every catalogue owner and schedules each
    /// currently blocked eligible turn. A failed scan reverts only the
    /// unsettled revision this call installed. A finished scan settles the
    /// revision it observed.
    pub async fn set_usage_limit_auto_continue(
        &self,
        enabled: bool,
        caller: ConversationCaller,
        facts: UsageLimitPreferenceFacts,
    ) -> Result<PreferenceUpdate, ConversationError> {
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
            return Ok(PreferenceUpdate::Refused { policies });
        };
        if stored {
            let mut visited = Vec::new();
            if let Err(error) = self
                .schedule_every_owner(&store, true, ScheduleActor::person(&caller), &mut visited)
                .await
            {
                if recorded.installed_enable {
                    let now = self.inner.clock.unix_milliseconds();
                    store
                        .revert_installed_preference(
                            recorded.revision,
                            ScheduleActor::system(),
                            now,
                        )
                        .await?;
                }
                self.publish_preference(&visited);
                return Err(error);
            }
            // A scan that finished settles the revision it saw, including one
            // a concurrent request installed. That request's later revert then
            // matches nothing.
            if !store.settle_installed_preference(recorded.revision).await? {
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
    pub(crate) async fn consider_usage_limit(
        &self,
        id: &ConversationId,
        enabled_by_person: bool,
        actor: ScheduleActor,
    ) {
        let Some(store) = self.scheduled_messages() else {
            return;
        };
        if let Err(error) = self
            .apply_usage_limit_plan(id, &store, enabled_by_person, actor)
            .await
        {
            tracing::warn!(conversation_id = %id, %error, "a usage-limit continuation could not be scheduled");
        }
    }

    async fn apply_usage_limit_plan(
        &self,
        id: &ConversationId,
        store: &Arc<dyn ScheduledMessageStore>,
        enabled_by_person: bool,
        actor: ScheduleActor,
    ) -> Result<(), ConversationError> {
        let Some(facts) = self.usage_limit_facts(id).await? else {
            return Ok(());
        };
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
            ContinuationPlan::Hold => {}
            ContinuationPlan::Accept { due_at_ms, cause } => {
                self.accept_usage_limit(store, &facts, due_at_ms, cause, actor, standing)
                    .await?;
            }
            ContinuationPlan::MoveDue { due_at_ms } => {
                store
                    .move_due(&id.to_string(), &facts_causal(&facts), due_at_ms, now_ms)
                    .await?;
            }
            ContinuationPlan::Defer { due_at_ms } => {
                store
                    .defer_claim(&id.to_string(), &facts_causal(&facts), due_at_ms, now_ms)
                    .await?;
            }
            ContinuationPlan::Cancel { cause } => {
                store
                    .cancel_open(&id.to_string(), cause, actor, now_ms)
                    .await?;
            }
            ContinuationPlan::Switch {
                cancel,
                due_at_ms,
                cause,
            } => {
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
                },
                at_ms,
            )
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
        for row in rows {
            self.deliver_scheduled(store.clone(), row).await;
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
    /// after [`catch_up_backoff`]. A preference read that fails leaves it
    /// pending the same way.
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
                                        let retry = catch_up_backoff(catch_up_failures);
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
                            let retry = catch_up_backoff(catch_up_failures);
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
            let wait = next_pass_wait(self.deliver_due_scheduled().await, catch_up_wait);
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
    ) {
        let Ok(id) = ConversationId::new(&row.conversation) else {
            return;
        };
        // A plan that does not commit leaves the row where it is. Submitting
        // it would deliver a continuation the model change had not cancelled.
        if self
            .apply_usage_limit_plan(&id, &store, false, ScheduleActor::system())
            .await
            .is_err()
        {
            return;
        }
        // Reconciliation above may cancel or defer while the row is still
        // `delivering`. The transition below is the decision to submit: a
        // cancel, model change, or later reset that wins first leaves this
        // false, and one that arrives after cannot undo it.
        let now = self.inner.clock.unix_milliseconds();
        let Ok(true) = store.begin_submit(&row.execution_id, now).await else {
            return;
        };
        self.finish_fenced_submit(store.clone(), id, &row).await;
        let _ = store.release_submit(&row.execution_id).await;
    }

    async fn finish_fenced_submit(
        &self,
        store: Arc<dyn ScheduledMessageStore>,
        id: ConversationId,
        row: &ScheduledMessage,
    ) {
        let lost = Arc::new(AtomicBool::new(false));
        let renew = {
            let store = store.clone();
            let execution_id = row.execution_id.clone();
            let clock = self.inner.clock.clone();
            let lost = lost.clone();
            tokio::spawn(async move {
                renew_while_held(store, execution_id, clock, lost).await;
            })
        };
        let _renew = StopRenew(renew);
        if self.execution_saved(&id, &row.execution_id).await {
            let now = self.inner.clock.unix_milliseconds();
            let _ = store
                .record_outcome(
                    &row.execution_id,
                    ScheduleSettlement::Delivered,
                    "delivered",
                    now,
                )
                .await;
            return;
        }
        let Some(commands) = self.commands() else {
            return;
        };
        // A stored caller or grant that cannot be rebuilt will never pass
        // submit. Refuse it once. The row stays out of `delivering`, so the
        // next pass adopts `submitting` only until this refusal is recorded.
        let (caller, initiation) = match (schedule_caller(row), continuation_initiation(row)) {
            (Ok(caller), Ok(initiation)) => (caller, initiation),
            _ => {
                let now = self.inner.clock.unix_milliseconds();
                let _ = store
                    .record_outcome(
                        &row.execution_id,
                        ScheduleSettlement::Refused,
                        "revoked",
                        now,
                    )
                    .await;
                return;
            }
        };
        // The store said this process no longer owns the row. Do not start a
        // second effect. A submit that has already run still records below.
        if lost.load(Ordering::SeqCst) {
            return;
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
                let now = self.inner.clock.unix_milliseconds();
                let _ = store
                    .record_outcome(
                        &row.execution_id,
                        ScheduleSettlement::Delivered,
                        "delivered",
                        now,
                    )
                    .await;
            }
            Err(error) => {
                if let Some((outcome, cause)) = settled_outcome(&error) {
                    let now = self.inner.clock.unix_milliseconds();
                    let _ = store
                        .record_outcome(&row.execution_id, outcome, cause, now)
                        .await;
                }
            }
        }
    }

    async fn execution_saved(&self, id: &ConversationId, execution_id: &str) -> bool {
        let session = super::conversation_session(id);
        let Ok(Some(committed)) = self.inner.storage.read_committed(session).await else {
            return false;
        };
        committed.snapshot().is_some_and(|snapshot| {
            snapshot
                .invocations
                .iter()
                .any(|invocation| invocation.request.execution_id.as_str() == execution_id)
        })
    }
}

/// How long to wait before the next catch-up attempt.
///
/// The first failure waits [`SCHEDULE_RETRY_MS`]. Each later failure doubles
/// that wait, up to [`CATCH_UP_RETRY_MAX_MS`].
fn catch_up_backoff(failures: u32) -> u64 {
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

/// The sooner of a delivery wait and a pending catch-up retry.
///
/// A catch-up that is still owed must not park on an empty schedule. A due
/// row must not wait out the whole catch-up backoff.
fn next_pass_wait(delivery: Option<u64>, catch_up: Option<u64>) -> Option<u64> {
    match (delivery, catch_up) {
        (Some(delivery), Some(retry)) => Some(delivery.min(retry)),
        (None, Some(retry)) => Some(retry),
        (delivery, None) => delivery,
    }
}

/// Renew `execution_id` until the store says this process does not own it.
///
/// `Ok(true)` waits [`FENCE_RENEW_EVERY_MS`] and renews again. `Err` waits
/// [`FENCE_RENEW_RETRY_MS`] and renews again, and leaves `lost` false. `Ok(false)`
/// sets `lost` and stops. A transient renew failure does not abandon a submit
/// that is still running.
async fn renew_while_held(
    store: Arc<dyn ScheduledMessageStore>,
    execution_id: String,
    clock: Arc<dyn Clock>,
    lost: Arc<AtomicBool>,
) {
    loop {
        tokio::time::sleep(Duration::from_millis(FENCE_RENEW_EVERY_MS)).await;
        loop {
            let now = clock.unix_milliseconds();
            match store.renew_fence(&execution_id, now).await {
                Ok(true) => break,
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
    let mut prompt = String::from(NOTICE);
    let room = max_bytes.saturating_sub(NOTICE.len());
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

    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use nessa_auth::application::ports::{AccessError, Clock};
    use nessa_auth::domain::{OrganizationId, PrincipalId};
    use nessa_sdk::application::agent_execution::commands::{
        CreationFuture, CreationReceipt, CreationStorage, CreationStorageLease, MutationReceipt,
    };

    use super::{
        renew_while_held, PreferenceReading, PreferenceUpdate, RecordedPreference, ScheduleActor,
        ScheduleDraft, ScheduleSettlement, ScheduledMessage, ScheduledMessageStore,
        FENCE_RENEW_EVERY_MS, FENCE_RENEW_RETRY_MS, SCHEDULE_RETRY_MS,
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

    fn caller() -> ConversationCaller {
        ConversationCaller {
            organization_id: OrganizationId::new("org").unwrap(),
            principal_id: PrincipalId::new("person").unwrap(),
            surface_id: "desktop".into(),
            action_id: "toggle".into(),
        }
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
        let provider = Arc::new(ProviderFactory::default());
        ConversationService::new(
            ConversationDependencies {
                access_policy: crate::conversation_test_support::access_policy(),
                agents: only(Arc::new(Provider::new(provider))),
                storage: Arc::new(InMemoryStorage::new()),
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
            }
        }

        /// Settle `revision` when it is still the enabled preference.
        fn settle_revision(&self, revision: u64) -> bool {
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
        fn claim_due(&self, _: u64, _: usize) -> ConversationFuture<'_, Vec<ScheduledMessage>> {
            if self.idle_delivery.load(Ordering::SeqCst) {
                Box::pin(async { Ok(Vec::new()) })
            } else {
                refused()
            }
        }
        fn next_due(&self, _: u64) -> ConversationFuture<'_, Option<u64>> {
            if self.idle_delivery.load(Ordering::SeqCst) {
                Box::pin(async { Ok(None) })
            } else {
                refused()
            }
        }
        fn begin_submit(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
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
            self.decisions
                .lock()
                .unwrap()
                .push((allowed, policies.to_vec()));
            if !allowed {
                let policies = policies.to_vec();
                let revision = self.revision.load(Ordering::SeqCst);
                return Box::pin(async move {
                    Ok(RecordedPreference {
                        update: PreferenceUpdate::Refused { policies },
                        revision,
                        installed_enable: false,
                    })
                });
            }
            let was = self.enabled.swap(enabled, Ordering::SeqCst);
            let installed_enable = !was && enabled;
            if was != enabled {
                self.revision.fetch_add(1, Ordering::SeqCst);
                self.settled.store(!installed_enable, Ordering::SeqCst);
                self.changes.lock().unwrap().push((enabled, actor.person));
            }
            let revision = self.revision.load(Ordering::SeqCst);
            Box::pin(async move {
                Ok(RecordedPreference {
                    update: PreferenceUpdate::Stored(enabled),
                    revision,
                    installed_enable,
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
            }
            Box::pin(async move { Ok(matches) })
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
        };
        service
            .finish_fenced_submit(
                store.clone(),
                ConversationId::new(conversation).unwrap(),
                &row,
            )
            .await;
        assert_eq!(
            store.outcomes.lock().unwrap().as_slice(),
            [("exec-1".into(), 9_000)]
        );
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
        ));
        tokio::time::sleep(Duration::from_millis(FENCE_RENEW_EVERY_MS + 1)).await;
        assert!(lost.load(Ordering::SeqCst));
        assert_eq!(schedule.renew_calls.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(FENCE_RENEW_EVERY_MS)).await;
        assert_eq!(schedule.renew_calls.load(Ordering::SeqCst), 1);
        task.abort();
    }

    #[test]
    fn catch_up_backoff_doubles_until_it_is_capped() {
        assert_eq!(super::catch_up_backoff(1), SCHEDULE_RETRY_MS);
        assert_eq!(super::catch_up_backoff(2), SCHEDULE_RETRY_MS * 2);
        assert_eq!(super::catch_up_backoff(3), SCHEDULE_RETRY_MS * 4);
        assert_eq!(super::catch_up_backoff(30), super::CATCH_UP_RETRY_MAX_MS);
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
}
