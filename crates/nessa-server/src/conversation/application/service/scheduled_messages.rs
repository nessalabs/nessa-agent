//! One scheduled continuation after a provider usage limit.
//!
//! The preference is one boolean for this gateway, default off. A row is the
//! durable one-shot message. Delivery calls [`ConversationService::submit_command`]
//! with the causal turn's initiation, so Cedar and a deleted conversation are
//! decided there. The compare-and-set from `delivering` to `submitting` is the
//! only permission to submit, and it takes a lease the other processes on this
//! file can see. A retryable submit, an outcome whose audit does not commit,
//! a reconciliation whose plan does not commit, and a delivery whose plan
//! does not commit are one retry: [`next_attempt`]. The next pass does not
//! enter that work before `retry_at_ms`. When the durable retry does not
//! commit, this process keeps the same attempt. A lost lease writes nothing.
//! An unread execution, and a `begin_submit` whose transaction does not
//! commit, write no `retry_at_ms` and are still that attempt while the row
//! is `delivering` or `submitting`. A committed outcome, a cancelled row,
//! and a missing row drop the attempt this process kept.
//! Cancel and defer lose to that transition. Selection changes share the
//! submission permit and seal a crash-left unattempted command before Applied.
//! Attempted and terminal command evidence retains its original meaning.
//! Cedar's allow and deny of the preference are each one audit before the
//! command result. That audit and the request's receipt commit together.
//! An enable stays pending until its scan settles or its rollback commits.
//! A retry of that receipt resumes the unfinished work. A finished receipt
//! is returned and is not applied again.

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
use nessa_sdk::application::agent_execution::commands::{
    CreationFuture, CreationInitializationFailure, CreationStorage, MutationBinding,
    MutationCoordinator, MutationFailure, MutationOperation, MutationOutcome, MutationPlan,
    MutationReceipt, MutationStage, MutationTarget,
};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, GrantInitiator, Initiation, Initiator, ToolChecking,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Which selection request invalidates a blocked continuation. The same kind
/// owns its durable request namespace and cancellation reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionChange {
    Model,
    Agent,
}

/// The original selection command, including the kind that owns its request id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRequest<'a> {
    pub change: SelectionChange,
    pub id: &'a str,
}

impl SelectionChange {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Agent => "agent",
        }
    }
    pub(crate) fn cause(self) -> ContinuationCause {
        match self {
            Self::Model => ContinuationCause::ModelChanged,
            Self::Agent => ContinuationCause::AgentChanged,
        }
    }
}

/// Who an audit row names. A provider limit is the system. The person is named
/// for preference changes, selection requests, and their cancellations.
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

    /// The original model request's caller, including during later recovery.
    pub(crate) fn model_request(
        request: &crate::conversation::application::ConversationModelRequest,
    ) -> Self {
        Self {
            person: true,
            organization_id: Some(request.organization_id.as_str().to_owned()),
            principal_id: Some(request.initiator_principal_id.as_str().to_owned()),
            surface_id: Some(request.initiator_surface_id.clone()),
            action_id: Some(request.request_id.clone()),
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

/// Immutable, non-content command facts retained when deletion scrubs a
/// continuation. Identity bounds and validation are owned by ActionContext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledCommandEvidence {
    actor: ActionContext,
    fingerprint: [u8; 32],
}

impl ScheduledCommandEvidence {
    pub(crate) fn capture(
        execution: &str,
        principal: &str,
        surface: &str,
        prompt: &str,
    ) -> Result<Self, ConversationError> {
        let fingerprint = super::mutation::submission_fingerprint(
            execution,
            &super::SubmittedMessage {
                text: prompt.into(),
                ..Default::default()
            },
            super::SubmissionMode::Queue,
        )?;
        Self::restore(execution, principal, surface, fingerprint)
    }

    pub(crate) fn restore(
        execution: &str,
        principal: &str,
        surface: &str,
        fingerprint: [u8; 32],
    ) -> Result<Self, ConversationError> {
        let actor = ActionContext::new(principal, surface, execution)
            .map_err(|_| ConversationError::InvalidInput)?;
        Ok(Self { actor, fingerprint })
    }

    pub(crate) fn principal(&self) -> &str {
        self.actor.principal_id()
    }
    pub(crate) fn surface(&self) -> &str {
        self.actor.surface_id()
    }
    pub(crate) fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }

    fn binding(
        &self,
        id: &ConversationId,
        execution: &str,
    ) -> Result<MutationBinding, ConversationError> {
        if self.actor.request_id() != execution {
            return Err(ConversationError::InvalidInput);
        }
        MutationBinding::new(
            self.actor.clone(),
            MutationOperation::Submit,
            super::conversation_session(id),
            execution,
            self.fingerprint,
        )
        .map_err(|_| ConversationError::InvalidInput)
    }
}

/// A continuation the store has accepted or is delivering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledMessage {
    pub command_evidence: Option<ScheduledCommandEvidence>,
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
/// are current only when that generation is still stored. An older generation
/// writes nothing, including when the incarnation and position still match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryGeneration {
    pub incarnation: String,
    pub position: u64,
    /// The fence generation this plan observed. 0 is an empty fence.
    pub generation: u64,
}

/// Everything a plan read that a later write can make stale.
///
/// A schedule-row change compares this token in one transaction: the history
/// generation, the preference revision, and the agent and model. The same
/// session history is not current by itself. A different agent or model,
/// observed at the stored generation, seals the next generation without a new
/// session position. An older generation writes nothing, even when the model
/// has changed back to the one this plan read. A preference revision that no
/// longer matches writes nothing and does not advance the fence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanToken {
    pub history: HistoryGeneration,
    pub preference_revision: u64,
    /// The agent the plan read. Fixed for a conversation until it changes.
    pub agent: String,
    /// The model the plan read. A change lives in conversation metadata and
    /// does not advance the session incarnation or position.
    pub model: String,
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
    /// that generation, and the agent and model, are still the ones stored.
    pub history: HistoryGeneration,
}

impl ScheduleDraft {
    /// The compare-and-set token this draft observed.
    pub(crate) fn token(&self) -> PlanToken {
        PlanToken {
            history: self.history.clone(),
            preference_revision: self.preference_revision,
            agent: self.agent.clone(),
            model: self.model.clone(),
        }
    }
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

/// An enable that is on and not yet settled, and the request that installed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsettledEnable {
    pub revision: u64,
    pub installed_request: String,
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
    /// This request's enable has not finished its scan or its rollback.
    /// The command does not return this. A retry resumes that work.
    Pending,
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
    /// The decision committed and the scan or rollback has not. A retry
    /// resumes that work. This is not success.
    Pending,
}

/// The private dataset of scheduled continuations.
pub trait ScheduledMessageStore: Send + Sync {
    /// The gateway preference and the revision of that read. Absent means off
    /// at revision 0.
    fn preference(&self) -> ConversationFuture<'_, PreferenceReading>;
    /// Record the preference. Turning it off cancels unsubmitted rows in the
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
    /// the draft's token is current: the preference revision is still enabled,
    /// and the history generation, agent, and model are still the ones stored.
    /// A finished causal turn, a revision a preference change has replaced, or
    /// an older generation returns `None`.
    fn accept(
        &self,
        draft: ScheduleDraft,
        at_ms: u64,
    ) -> ConversationFuture<'_, Option<ScheduledMessage>>;
    /// Move a scheduled row's due time and audit that move with the status change.
    /// `token` is compared in that same transaction. A miss writes nothing and
    /// does not advance the fence.
    fn move_due(
        &self,
        conversation: &str,
        causal_execution: &str,
        due_at_ms: u64,
        token: &PlanToken,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool>;
    /// Release a delivering claim back to scheduled at a later due time.
    /// `token` is compared in that same transaction. A miss writes nothing and
    /// does not advance the fence.
    fn defer_claim(
        &self,
        conversation: &str,
        causal_execution: &str,
        due_at_ms: u64,
        token: &PlanToken,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool>;
    /// Cancel the open row for one conversation. `token` is compared in that
    /// same transaction. A miss writes nothing and does not advance the fence.
    /// The preference does not have to be on: a plan that read this revision
    /// while it was off is the backstop that cancels the row.
    fn cancel_open(
        &self,
        conversation: &str,
        cause: ContinuationCause,
        actor: ScheduleActor,
        token: &PlanToken,
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
    /// The fence generation stored for `conversation`, or 0 when none is sealed.
    /// Read this before the session, then put it on [`HistoryGeneration`].
    /// The default is an empty fence.
    fn history_generation(&self, conversation: &str) -> ConversationFuture<'_, u64> {
        let _ = conversation;
        Box::pin(async { Ok(0) })
    }
    /// Invalidate the blocked causal turn when an applied selection request first
    /// reconciles, including with preference off or no schedule. The request's
    /// receipt, token compare, cancellation and audit commit together. A repeat
    /// reads that receipt without invalidating a later turn. False is a stale
    /// token; absence of a blocked turn is recorded as an empty causal id.
    fn invalidate_usage_limit(
        &self,
        request: SelectionRequest<'_>,
        conversation: &str,
        causal_execution: Option<&str>,
        token: &PlanToken,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool>;
    /// Original causal turn recorded by this selection request. Used to seal
    /// only that request's submitting command when a pending request recovers.
    fn selection_invalidation_causal(
        &self,
        request: SelectionRequest<'_>,
        conversation: &str,
    ) -> ConversationFuture<'_, Option<String>> {
        let _ = (request, conversation);
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    /// Seal `token` when it is still current. A different revision writes
    /// nothing, including the fence. A later position, a different
    /// incarnation, or a different agent or model replaces the fence only
    /// when `token` observed the generation now stored, and that number then
    /// advances. An older generation writes nothing and returns false,
    /// including when the incarnation and position still match. The same
    /// history and the same agent and model are already sealed and return
    /// true. When `dismiss_open` is true and the token matches, an open
    /// `scheduled` or `delivering` row is cancelled in that same transaction.
    /// The default seals nothing.
    fn note_history(
        &self,
        conversation: &str,
        token: &PlanToken,
        dismiss_open: bool,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool> {
        let _ = (conversation, token, dismiss_open, actor, at_ms);
        Box::pin(async { Ok(true) })
    }
    /// Mark this request's pending receipt failed. A stored or refused
    /// receipt is left as it is. The preference and any other request stay
    /// as they are. The default records nothing.
    fn fail_preference_command(&self, request_id: &str) -> ConversationFuture<'_, ()> {
        let _ = request_id;
        Box::pin(async { Ok(()) })
    }
    /// Mark this request's pending receipt stored. False when it was not
    /// pending. The default reports that the receipt is already finished.
    fn complete_preference_command(&self, request_id: &str) -> ConversationFuture<'_, bool> {
        let _ = request_id;
        Box::pin(async { Ok(true) })
    }
    /// The enable that is still unsettled, when one is installed. The default
    /// is none, for a double that does not record an install.
    fn unsettled_enable(&self) -> ConversationFuture<'_, Option<UnsettledEnable>> {
        Box::pin(async { Ok(None) })
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
    /// Blank prompt and payload/grant identifiers for `conversation`, retaining
    /// non-content canonical command evidence. Unsubmitted rows become cancelled;
    /// submitting rows retain their status until leased command receipt recovery.
    /// Terminal rows keep theirs. One audit per row still holding payload;
    /// a second scrub writes nothing. Older audit rows stay.
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
        PreferenceCommandOutcome::Failed | PreferenceCommandOutcome::Pending => {
            Err(ConversationError::Unavailable)
        }
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
    /// Cedar or any write. A request that already finished returns that
    /// result and does not apply again. A request whose scan or rollback has
    /// not finished resumes that work and does not report success. Otherwise
    /// Cedar decides. Off cancels unsubmitted rows as this person; submitted
    /// rows recover their command evidence before retirement. On scans every
    /// catalogue owner and schedules each currently blocked eligible turn.
    /// The receipt stays pending until that scan settles the revision, or
    /// until the rollback of the unsettled revision this call installed
    /// commits. A revert that does not commit leaves the receipt pending.
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
            if matches!(saved.outcome, PreferenceCommandOutcome::Pending) {
                return self
                    .resume_pending_preference(&store, enabled, &caller, &saved)
                    .await;
            }
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
        match recorded.update {
            PreferenceUpdate::Refused { .. } => Ok(recorded.update),
            PreferenceUpdate::Pending => {
                let Some(saved) = store.preference_command(&caller.action_id).await? else {
                    return Err(ConversationError::Unavailable);
                };
                self.resume_pending_preference(&store, enabled, &caller, &saved)
                    .await
            }
            PreferenceUpdate::Stored(false) => {
                if !recorded.replayed {
                    self.publish_preference(&[]);
                }
                Ok(PreferenceUpdate::Stored(false))
            }
            PreferenceUpdate::Stored(true) => {
                if recorded.replayed {
                    return Ok(PreferenceUpdate::Stored(true));
                }
                self.finish_enable_scan(
                    &store,
                    &caller,
                    recorded.revision,
                    recorded.installed_enable,
                )
                .await
            }
        }
    }

    /// Continue an enable whose receipt is still pending.
    async fn resume_pending_preference(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        enabled: bool,
        caller: &ConversationCaller,
        saved: &PreferenceCommandReceipt,
    ) -> Result<PreferenceUpdate, ConversationError> {
        if saved.organization_id != caller.organization_id.as_str()
            || saved.principal_id != caller.principal_id.as_str()
            || saved.surface_id != caller.surface_id
            || saved.enabled != enabled
        {
            return Err(ConversationError::RequestConflict);
        }
        let reading = store.preference().await?;
        if !reading.enabled {
            store.fail_preference_command(&caller.action_id).await?;
            return Err(ConversationError::Unavailable);
        }
        let unsettled = store.unsettled_enable().await?;
        let installed = unsettled
            .as_ref()
            .is_some_and(|row| row.installed_request == caller.action_id);
        let revision = match &unsettled {
            Some(row) if installed => row.revision,
            _ => reading.revision,
        };
        self.finish_enable_scan(store, caller, revision, installed)
            .await
    }

    /// Scan every owner for an enable, then settle it or roll it back.
    ///
    /// The receipt stays pending when the revert or the failure write does
    /// not commit. It becomes stored only after the scan settles.
    async fn finish_enable_scan(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        caller: &ConversationCaller,
        revision: u64,
        installed_enable: bool,
    ) -> Result<PreferenceUpdate, ConversationError> {
        let mut visited = Vec::new();
        if let Err(error) = self
            .schedule_every_owner(store, true, ScheduleActor::person(caller), &mut visited)
            .await
        {
            if installed_enable {
                let now = self.inner.clock.unix_milliseconds();
                match store
                    .revert_installed_preference(revision, ScheduleActor::system(), now)
                    .await
                {
                    Err(revert_error) => {
                        self.publish_preference(&visited);
                        return Err(revert_error);
                    }
                    Ok(true) => {
                        self.publish_preference(&visited);
                        return Err(error);
                    }
                    Ok(false) => {
                        store.fail_preference_command(&caller.action_id).await?;
                    }
                }
            } else {
                store.fail_preference_command(&caller.action_id).await?;
            }
            self.publish_preference(&visited);
            return Err(error);
        }
        // A scan that finished settles the revision it saw, including one
        // a concurrent request installed. That request's later revert then
        // matches nothing.
        if !store.settle_installed_preference(revision).await? {
            store.fail_preference_command(&caller.action_id).await?;
            self.publish_preference(&visited);
            return Err(ConversationError::Unavailable);
        }
        if !store.complete_preference_command(&caller.action_id).await? {
            self.publish_preference(&visited);
            return Err(ConversationError::Unavailable);
        }
        self.publish_preference(&visited);
        Ok(PreferenceUpdate::Stored(true))
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

    pub(crate) async fn invalidate_selection_usage_limit(
        &self,
        change: SelectionChange,
        id: &ConversationId,
        request_id: &str,
        actor: ScheduleActor,
    ) -> Result<(), ConversationError> {
        let Some(store) = self.scheduled_messages() else {
            return Ok(());
        };
        let request = SelectionRequest {
            change,
            id: request_id,
        };
        let generation = store.history_generation(&id.to_string()).await?;
        let (history, agent, model, causal) = match self.usage_limit_facts(id).await? {
            Some(facts) => (
                facts.history,
                facts.agent,
                facts.model,
                facts.observed.map(|limit| limit.causal_execution),
            ),
            None => {
                let record = super::load_conversation(self.inner.metadata.as_ref(), id)
                    .await?
                    .ok_or(ConversationError::NotFound)?;
                (
                    HistoryGeneration {
                        incarnation: String::new(),
                        position: 0,
                        generation,
                    },
                    record
                        .agent()
                        .map(|agent| agent.name().to_owned())
                        .unwrap_or_default(),
                    record.model().as_str().to_owned(),
                    None,
                )
            }
        };
        let history = HistoryGeneration {
            generation,
            ..history
        };
        let standing = store.preference().await?;
        let token = PlanToken {
            history,
            preference_revision: standing.revision,
            agent,
            model,
        };
        if !store
            .invalidate_usage_limit(
                request,
                &id.to_string(),
                causal.as_deref(),
                &token,
                actor,
                self.inner.clock.unix_milliseconds(),
            )
            .await?
        {
            return Err(match change {
                SelectionChange::Model => ConversationError::ModelChangeUncertain,
                SelectionChange::Agent => ConversationError::RequestConflict,
            });
        }
        // A crash can leave a pre-admission submitting row behind. Seal its
        // original command before Applied, even if the person switches back
        // before delivery recovers. Terminal and attempted receipts are retained.
        if let Some(row) = store.open(&id.to_string()).await? {
            if store.status(&row.execution_id).await?.as_deref() == Some("submitting")
                && store
                    .selection_invalidation_causal(request, &id.to_string())
                    .await?
                    .as_deref()
                    == Some(row.causal_execution.as_str())
            {
                let commands = self.commands().ok_or(ConversationError::Unavailable)?;
                let binding = scheduled_submit_binding(id, &row)?;
                let plan = match change {
                    SelectionChange::Model => MutationPlan::ModelChanged,
                    SelectionChange::Agent => MutationPlan::AgentChanged,
                };
                match MutationCoordinator::new(commands)
                    .execute(binding, Arc::new(CancelUnattemptedSubmission(plan)))
                    .await
                {
                    Ok(_) | Err(MutationFailure::Interrupted(_)) => {}
                    Err(_) => return Err(ConversationError::Unavailable),
                }
            }
        }
        Ok(())
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
        let (failures, retry_at) = next_attempt(now, previous);
        let wait = retry_at.saturating_sub(now);
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
        let mut failures = 0;
        if let Ok(rows) = store.reconcile_retries().await {
            if let Some(row) = rows
                .into_iter()
                .find(|row| row.conversation == conversation)
            {
                failures = row.failures;
            }
        }
        failures.max(
            self.memory_reconcile(conversation)
                .map(|(failures, _)| failures)
                .unwrap_or(0),
        )
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

    fn submit_retries(&self) -> Vec<(String, u64)> {
        self.inner
            .submit_retry_owed
            .lock()
            .expect("submit retry")
            .iter()
            .map(|(execution_id, (_, retry_at_ms))| (execution_id.clone(), *retry_at_ms))
            .collect()
    }

    fn submit_retry_failures(&self, execution_id: &str) -> u32 {
        self.inner
            .submit_retry_owed
            .lock()
            .expect("submit retry")
            .get(execution_id)
            .map(|(failures, _)| *failures)
            .unwrap_or(0)
    }

    /// How long until this execution's uncommitted retry, when it is still ahead.
    fn submit_wait_remaining(&self, execution_id: &str) -> Option<u64> {
        let now = self.inner.clock.unix_milliseconds();
        self.inner
            .submit_retry_owed
            .lock()
            .expect("submit retry")
            .get(execution_id)
            .filter(|(_, retry_at)| *retry_at > now)
            .map(|(_, retry_at)| retry_at - now)
    }

    fn remember_submit_retry(&self, execution_id: &str, failures: u32, retry_at_ms: u64) {
        self.inner
            .submit_retry_owed
            .lock()
            .expect("submit retry")
            .insert(execution_id.to_owned(), (failures, retry_at_ms));
    }

    fn forget_submit_retry(&self, execution_id: &str) {
        self.inner
            .submit_retry_owed
            .lock()
            .expect("submit retry")
            .remove(execution_id);
    }

    /// The attempt this process kept applies only while the row can still
    /// be retried. `delivering` and `submitting` keep it. A cancel, a
    /// terminal status, and a missing row drop it. A status read that fails
    /// leaves it.
    async fn retire_finished_submit_retries(&self, store: &Arc<dyn ScheduledMessageStore>) {
        for (execution_id, _) in self.submit_retries() {
            match store.status(&execution_id).await {
                Ok(Some(status)) if status == "delivering" || status == "submitting" => {}
                Ok(_) => self.forget_submit_retry(&execution_id),
                Err(error) => {
                    tracing::warn!(
                        %execution_id,
                        %error,
                        "a scheduled submit retry could not be read; the attempt stays"
                    );
                }
            }
        }
    }

    /// The next attempt for a submit or an outcome whose durable retry did
    /// not commit. Nothing is written on the row. The loop waits this long
    /// and does not enter the submit again before then.
    fn note_uncommitted_submit(&self, row: &ScheduledMessage) -> u64 {
        let now = self.inner.clock.unix_milliseconds();
        let previous = row
            .retry_failures
            .max(self.submit_retry_failures(&row.execution_id));
        let (failures, retry_at) = next_attempt(now, previous);
        self.remember_submit_retry(&row.execution_id, failures, retry_at);
        retry_at.saturating_sub(now)
    }

    /// How long until `conversation`'s reconcile debt, when that retry is
    /// still ahead. A durable debt and a debt this process is still holding
    /// both count. The later of the two wins, so a write the database
    /// refused is not retried on an older stored time.
    async fn reconcile_wait_remaining(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        conversation: &str,
    ) -> Option<u64> {
        let now = self.inner.clock.unix_milliseconds();
        let mut retry_at = self
            .memory_reconcile(conversation)
            .map(|(_, retry_at)| retry_at)
            .filter(|retry_at| *retry_at > now);
        if let Ok(rows) = store.reconcile_retries().await {
            if let Some(row) = rows
                .into_iter()
                .find(|row| row.conversation == conversation)
            {
                if row.retry_at_ms > now {
                    retry_at = Some(retry_at.map_or(row.retry_at_ms, |at| at.max(row.retry_at_ms)));
                }
            }
        }
        retry_at.map(|retry_at| retry_at - now)
    }

    /// How long until the soonest reconcile debt that is still ahead.
    /// [`SCHEDULE_RETRY_MS`] when the debts could not be read.
    async fn soonest_reconcile_hold(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        now: u64,
    ) -> Option<u64> {
        let mut soonest = None;
        match store.reconcile_retries().await {
            Ok(rows) => {
                for row in rows {
                    soonest = sooner_hold(soonest, row.retry_at_ms, now);
                }
            }
            Err(error) => {
                tracing::warn!(%error, "usage-limit reconcile debts could not be read");
                soonest = Some(SCHEDULE_RETRY_MS);
            }
        }
        for owed in self.memory_reconcile_debts() {
            soonest = sooner_hold(soonest, owed.retry_at_ms, now);
        }
        for (_, retry_at) in self.submit_retries() {
            soonest = sooner_hold(soonest, retry_at, now);
        }
        soonest
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
            if let Some(existing) = debts
                .iter_mut()
                .find(|row| row.conversation == owed.conversation)
            {
                // A note the database refused is the later attempt. The stored
                // row is the previous one and must not be entered again.
                if owed.retry_at_ms >= existing.retry_at_ms {
                    existing.failures = existing.failures.max(owed.failures);
                    existing.retry_at_ms = owed.retry_at_ms;
                }
            } else {
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
                    let wait = self
                        .note_reconcile_failure(&store, &debt.conversation)
                        .await;
                    soonest = Some(soonest.map_or(wait, |current| current.min(wait)));
                    tracing::warn!(
                        conversation = %debt.conversation,
                        %error,
                        retry_ms = wait,
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
        let token = PlanToken {
            history: facts.history.clone(),
            preference_revision: standing.revision,
            agent: facts.agent.clone(),
            model: facts.model.clone(),
        };
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
                // No row was kept. Seal this token and dismiss a row a stale
                // accept may have inserted after the plan was read. An empty
                // incarnation was not a committed history and does not move
                // the fence.
                if !token.history.incarnation.is_empty() {
                    store
                        .note_history(
                            &facts.conversation,
                            &token,
                            open.is_none(),
                            actor.clone(),
                            now_ms,
                        )
                        .await?;
                }
            }
            ContinuationPlan::Accept { due_at_ms, cause } => {
                self.accept_usage_limit(store, &facts, due_at_ms, cause, actor, standing)
                    .await?;
            }
            ContinuationPlan::MoveDue { due_at_ms } => {
                store
                    .move_due(
                        &id.to_string(),
                        &facts_causal(&facts),
                        due_at_ms,
                        &token,
                        now_ms,
                    )
                    .await?;
            }
            ContinuationPlan::Defer { due_at_ms } => {
                store
                    .defer_claim(
                        &id.to_string(),
                        &facts_causal(&facts),
                        due_at_ms,
                        &token,
                        now_ms,
                    )
                    .await?;
            }
            ContinuationPlan::Cancel { cause } => {
                store
                    .cancel_open(&id.to_string(), cause, actor, &token, now_ms)
                    .await?;
            }
            ContinuationPlan::Switch {
                due_at_ms, cause, ..
            } => {
                // Accept cancels the open row and inserts the replacement in
                // one transaction, against the token it read.
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
        self.retire_finished_submit_retries(&store).await;
        let now = self.inner.clock.unix_milliseconds();
        if store.preference().await.is_err() {
            return Some(SCHEDULE_RETRY_MS);
        }
        let rows = match store.claim_due(now, 8).await {
            Ok(rows) => rows,
            Err(_) => return Some(SCHEDULE_RETRY_MS),
        };
        let mut claimed = 0_usize;
        let mut held = 0_usize;
        for row in rows {
            claimed += 1;
            match self.deliver_scheduled(store.clone(), row).await {
                DeliveredStep::Held => held += 1,
                DeliveredStep::Quiet => {}
            }
        }
        let now = self.inner.clock.unix_milliseconds();
        let next = match store.next_due(now).await {
            Ok(next) => next,
            Err(_) => return Some(SCHEDULE_RETRY_MS),
        };
        let mut due = next.map(|at| at.saturating_sub(now).max(1));
        // A delivering row whose plan did not commit is not due until its
        // reconcile retry. `next_due` reporting now is that row when every
        // claim this pass was held, including a debt the database refused.
        if held > 0 && held == claimed && matches!(due, Some(wait) if wait <= 1) {
            due = None;
        }
        let hold = self.soonest_reconcile_hold(&store, now).await;
        match (due, hold) {
            (Some(due), Some(hold)) => Some(due.min(hold)),
            (Some(due), None) => Some(due),
            (None, Some(hold)) => Some(hold),
            (None, None) => None,
        }
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
    ) -> DeliveredStep {
        let Ok(id) = ConversationId::new(&row.conversation) else {
            return DeliveredStep::Quiet;
        };
        // A plan that does not commit leaves the row `delivering`. Submitting
        // it would deliver a continuation the model change had not cancelled.
        // The row is not due again until the reconcile debt's retry time, on
        // the same backoff as a settlement whose plan did not commit.
        if self
            .reconcile_wait_remaining(&store, &row.conversation)
            .await
            .is_some()
            || self.submit_wait_remaining(&row.execution_id).is_some()
        {
            return DeliveredStep::Held;
        }
        // Shared with model/agent changes; transferred to the supervised enqueue.
        let admission = self.inner.admission.clone().read_owned().await;
        if self.inner.retirement.get().is_some() {
            return DeliveredStep::Held;
        }
        let selection = Arc::new(super::mutation::SubmissionPermit {
            _admission: admission,
            _selection: self.inner.mode_changes.lock(&id).await,
        });
        let enabled = match store.preference().await {
            Ok(reading) => reading.enabled,
            Err(_) => {
                self.note_uncommitted_submit(&row);
                return DeliveredStep::Held;
            }
        };
        let submitting = match store.status(&row.execution_id).await {
            Ok(Some(status)) => status == "submitting",
            Ok(None) => false,
            Err(_) => {
                self.note_uncommitted_submit(&row);
                return DeliveredStep::Held;
            }
        };
        if !submitting && self.recover_scheduled_selection(&id, &row).await.is_err() {
            self.note_uncommitted_submit(&row);
            return DeliveredStep::Held;
        }
        if enabled
            && !submitting
            && !row.prompt.is_empty()
            && self
                .apply_usage_limit_plan(&id, &store, false, ScheduleActor::system())
                .await
                .is_err()
        {
            let wait = self.note_reconcile_failure(&store, &row.conversation).await;
            tracing::warn!(
                conversation = %row.conversation,
                retry_ms = wait,
                "a usage-limit delivery could not reconcile; the row stays delivering"
            );
            return DeliveredStep::Held;
        }
        // Reconciliation above may cancel or defer while the row is still
        // `delivering`. The transition below is the decision to submit: a
        // cancel or later reset that wins first leaves this false. Selection
        // changes wait for this permit; crash recovery seals unattempted work.
        let now = self.inner.clock.unix_milliseconds();
        match store.begin_submit(&row.execution_id, now).await {
            Ok(true) => {}
            // A missed compare-and-set wrote nothing. `next_due` owns the wait.
            Ok(false) => return DeliveredStep::Quiet,
            Err(error) => {
                let wait = self.note_uncommitted_submit(&row);
                tracing::warn!(
                    execution_id = %row.execution_id,
                    %error,
                    retry_ms = wait,
                    "a scheduled submit could not be claimed; the row stays delivering"
                );
                return DeliveredStep::Held;
            }
        }
        let lease_until = now.saturating_add(FENCE_LEASE_MS);
        let held = self
            .finish_fenced_submit(store.clone(), id, &row, lease_until, selection)
            .await;
        let _ = store.release_submit(&row.execution_id).await;
        if held {
            DeliveredStep::Held
        } else {
            DeliveredStep::Quiet
        }
    }

    async fn finish_fenced_submit(
        &self,
        store: Arc<dyn ScheduledMessageStore>,
        id: ConversationId,
        row: &ScheduledMessage,
        lease_until: u64,
        selection: Arc<super::mutation::SubmissionPermit>,
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
        let Some(commands) = self.commands() else {
            if lost.load(Ordering::SeqCst) {
                return false;
            }
            return !self.hold_submit_retry(&store, row).await;
        };
        let receipt = self
            .recover_submission_receipt(commands.clone(), &id, row)
            .await;
        if lost.load(Ordering::SeqCst) {
            return false;
        }
        match receipt {
            Ok(Some((outcome, cause))) => {
                return self
                    .persist_observed_outcome(&store, row, outcome, cause)
                    .await
            }
            Ok(None) => {}
            Err(ConversationError::Storage(_)) => {
                self.note_uncommitted_submit(row);
                return true;
            }
            Err(_) => return !self.hold_submit_retry(&store, row).await,
        }
        if row.prompt.is_empty() {
            return self
                .persist_observed_outcome(
                    &store,
                    row,
                    ScheduleSettlement::Unconfirmed,
                    ContinuationCause::ConversationDeleted.as_str(),
                )
                .await;
        }
        // Recover pending metadata before comparing selection, including when
        // consent is off. Terminal command evidence above keeps its original cause.
        match self.recover_scheduled_selection(&id, row).await {
            Err(_) => return !self.hold_submit_retry(&store, row).await,
            Ok(false) => {}
            Ok(true) => {
                // Recovery may just have sealed this original command. Read
                // its cause even when the selected strings now match again.
                let receipt = self
                    .recover_submission_receipt(commands.clone(), &id, row)
                    .await;
                if lost.load(Ordering::SeqCst) {
                    return false;
                }
                match receipt {
                    Ok(Some((outcome, cause))) => {
                        return self
                            .persist_observed_outcome(&store, row, outcome, cause)
                            .await
                    }
                    Ok(None) => {}
                    Err(_) => return !self.hold_submit_retry(&store, row).await,
                }
            }
        }
        match store.preference().await {
            Ok(reading) if !reading.enabled => {
                return self
                    .retire_scheduled_submission(
                        &store,
                        commands,
                        id,
                        row,
                        &lost,
                        MutationPlan::NoEffect,
                    )
                    .await;
            }
            Ok(_) => {}
            Err(_) => return !self.hold_submit_retry(&store, row).await,
        }
        let change = match super::load_conversation(self.inner.metadata.as_ref(), &id).await {
            Ok(Some(record))
                if record
                    .agent()
                    .is_some_and(|agent| agent.name() != row.agent) =>
            {
                Some(MutationPlan::AgentChanged)
            }
            Ok(Some(record)) if record.model().as_str() != row.model => {
                Some(MutationPlan::ModelChanged)
            }
            Ok(_) => None,
            Err(_) => return !self.hold_submit_retry(&store, row).await,
        };
        if let Some(change) = change {
            return self
                .retire_scheduled_submission(&store, commands, id, row, &lost, change)
                .await;
        }
        // A stored caller or grant that cannot be rebuilt will never pass
        // submit. Refuse it. The row stays out of `delivering`. When the
        // refusal's audit commits, the next pass adopts `submitting` only
        // until that record is visible. When the audit does not commit, the
        // row stays `submitting` until a bounded retry and that same refusal
        // is recorded then.
        let (caller, initiation) = match (schedule_caller(row), continuation_initiation(row)) {
            (Ok(caller), Ok(initiation)) => (caller, initiation),
            _ => {
                if lost.load(Ordering::SeqCst) {
                    return false;
                }
                return self
                    .persist_observed_outcome(&store, row, ScheduleSettlement::Refused, "revoked")
                    .await;
            }
        };
        // The store said this process no longer owns the row, or the lease it
        // wrote has expired. Do not start a second effect, and do not move a
        // retry time on a row we do not own.
        if lost.load(Ordering::SeqCst) {
            return false;
        }
        match self
            .submit_command_permitted(
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
                Some(selection),
            )
            .await
        {
            Ok(_) => {
                if lost.load(Ordering::SeqCst) {
                    return false;
                }
                self.persist_observed_outcome(
                    &store,
                    row,
                    ScheduleSettlement::Delivered,
                    "delivered",
                )
                .await
            }
            Err(error) => {
                if lost.load(Ordering::SeqCst) {
                    return false;
                }
                if let Some((outcome, cause)) = settled_outcome(&error) {
                    self.persist_observed_outcome(&store, row, outcome, cause)
                        .await
                } else {
                    !self.hold_submit_retry(&store, row).await
                }
            }
        }
    }

    /// Called under the selection lock, before fresh eligibility classification.
    async fn recover_scheduled_selection(
        &self,
        id: &ConversationId,
        row: &ScheduledMessage,
    ) -> Result<bool, ConversationError> {
        let caller = schedule_caller(row)?;
        let mut recovered = false;
        if self
            .inner
            .metadata
            .pending_model_change(id)
            .await?
            .is_some()
        {
            recovered = true;
            self.recover_model_change(id, &caller).await?;
            self.refuse_pending_model_change(id, &caller).await?;
        }
        if self
            .inner
            .metadata
            .pending_agent_switch(id)
            .await?
            .is_some()
        {
            recovered = true;
            self.resolve(id, &caller).await?;
            self.finish_agent_switch_before_submission(id).await?;
        }
        Ok(recovered)
    }

    /// Recover retained command evidence before reading current preference: a
    /// later enable does not replace the reason a previous retirement recorded.
    async fn recover_submission_receipt(
        &self,
        commands: Arc<dyn CreationStorage>,
        id: &ConversationId,
        row: &ScheduledMessage,
    ) -> Result<Option<(ScheduleSettlement, &'static str)>, ConversationError> {
        let binding = scheduled_submit_binding(id, row)?;
        let receipt = MutationCoordinator::new(commands)
            .lookup(
                &binding,
                &CancelUnattemptedSubmission(MutationPlan::NoEffect),
            )
            .await
            .map_err(|_| ConversationError::Unavailable)?;
        // A terminal command result describes after-save refusals more exactly
        // than the existence of a saved invocation. Recover it before history.
        if let Some(receipt) = &receipt {
            if receipt.stage() == MutationStage::Settled {
                return submission_receipt_outcome(receipt)
                    .map(Some)
                    .ok_or(ConversationError::Unavailable);
            }
        }
        if self.execution_saved(id, &row.execution_id).await? {
            return Ok(Some((ScheduleSettlement::Delivered, "delivered")));
        }
        match receipt {
            None => Ok(None),
            Some(receipt) if receipt.stage() == MutationStage::Accepted => Ok(None),
            Some(receipt) => submission_receipt_outcome(&receipt)
                .map(Some)
                .ok_or(ConversationError::Unavailable),
        }
    }

    /// A disabled preference cannot erase an admitted command. Read its durable
    /// receipt under the command lease, sealing an unattempted command with
    /// RefusedBeforeAttempt. Then re-read the execution: the original command may have
    /// finished while recovery waited for its command lease.
    async fn retire_scheduled_submission(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        commands: Arc<dyn CreationStorage>,
        id: ConversationId,
        row: &ScheduledMessage,
        lost: &AtomicBool,
        plan: MutationPlan,
    ) -> bool {
        let recovered = async {
            let binding = scheduled_submit_binding(&id, row)?;
            let receipt = match MutationCoordinator::new(commands)
                .execute(binding, Arc::new(CancelUnattemptedSubmission(plan)))
                .await
            {
                Ok(receipt) => receipt,
                Err(MutationFailure::Interrupted(receipt)) => *receipt,
                Err(_) => return Err(ConversationError::Unavailable),
            };
            if receipt.stage() == MutationStage::Settled {
                return Ok(submission_receipt_outcome(&receipt));
            }
            if self.execution_saved(&id, &row.execution_id).await? {
                return Ok(Some((ScheduleSettlement::Delivered, "delivered")));
            }
            Ok::<_, ConversationError>(submission_receipt_outcome(&receipt))
        }
        .await;
        if lost.load(Ordering::SeqCst) {
            return false;
        }
        match recovered {
            Ok(Some((outcome, cause))) => {
                self.persist_observed_outcome(store, row, outcome, cause)
                    .await
            }
            Ok(None) | Err(_) => !self.hold_submit_retry(store, row).await,
        }
    }

    /// The outcome is already known. Commit it, or leave the row `submitting`
    /// until a bounded retry.
    ///
    /// `false` when the outcome committed, the row is gone, or the retry time
    /// committed: [`ScheduledMessageStore::next_due`] then carries the wait.
    /// `true` when the outcome did not commit and the retry time did not
    /// either. [`next_attempt`] still names the wait, kept in this process,
    /// and the submit is not entered again before it. A lost compare-and-set
    /// writes no status and no audit, and is not a debt.
    async fn persist_observed_outcome(
        &self,
        store: &Arc<dyn ScheduledMessageStore>,
        row: &ScheduledMessage,
        outcome: ScheduleSettlement,
        cause: &str,
    ) -> bool {
        let now = self.inner.clock.unix_milliseconds();
        match store
            .record_outcome(&row.execution_id, outcome, cause, now)
            .await
        {
            Ok(()) => {
                // The schedule is terminal even when the conversation itself
                // did not change. Subscribers re-read auto-resuming. The
                // attempt this process kept for a retry that did not commit
                // is finished.
                self.forget_submit_retry(&row.execution_id);
                if let Ok(id) = ConversationId::new(&row.conversation) {
                    self.inner.live_changes.publish(&id);
                }
                false
            }
            Err(ConversationError::NotFound) => {
                self.forget_submit_retry(&row.execution_id);
                false
            }
            Err(error) => {
                tracing::warn!(
                    execution_id = %row.execution_id,
                    %error,
                    "a scheduled outcome could not be recorded; the row waits for another attempt"
                );
                !self.hold_submit_retry(store, row).await
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
        let previous = row
            .retry_failures
            .max(self.submit_retry_failures(&row.execution_id));
        let (failures, retry_at) = next_attempt(now, previous);
        match store
            .defer_submit(&row.execution_id, failures, retry_at, now)
            .await
        {
            Ok(true) => {
                self.forget_submit_retry(&row.execution_id);
                true
            }
            Ok(false) => {
                self.remember_submit_retry(&row.execution_id, failures, retry_at);
                tracing::warn!(
                    execution_id = %row.execution_id,
                    retry_ms = retry_at.saturating_sub(now),
                    "a retryable scheduled submission could not be deferred"
                );
                false
            }
            Err(error) => {
                self.remember_submit_retry(&row.execution_id, failures, retry_at);
                tracing::warn!(
                    execution_id = %row.execution_id,
                    %error,
                    retry_ms = retry_at.saturating_sub(now),
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

/// Retire only commands whose journal has not admitted their original effect.
/// The coordinator owns the receipt/lease ordering; Attempted and Settled are
/// recovered before this target can classify an unattempted command.
struct CancelUnattemptedSubmission(MutationPlan);

impl MutationTarget for CancelUnattemptedSubmission {
    type Error = ConversationError;

    fn check(
        &self,
        _: &MutationBinding,
        _: Option<MutationStage>,
    ) -> CreationFuture<'_, (), Self::Error> {
        // This internal recovery only records no effect and never admits a
        // conversation action. Original admission evidence remains authoritative.
        Box::pin(async { Ok(()) })
    }

    fn classify(&self, _: &MutationBinding) -> CreationFuture<'_, MutationPlan, Self::Error> {
        Box::pin(async { Ok(self.0) })
    }

    fn effect(
        &self,
        _: &MutationBinding,
    ) -> CreationFuture<'_, MutationOutcome, CreationInitializationFailure<Self::Error>> {
        Box::pin(async {
            Err(CreationInitializationFailure::Target(
                ConversationError::Unavailable,
            ))
        })
    }
}

fn submission_receipt_outcome(
    receipt: &MutationReceipt,
) -> Option<(ScheduleSettlement, &'static str)> {
    match (receipt.stage(), receipt.outcome()) {
        (MutationStage::Attempted, _) => Some((
            ScheduleSettlement::Unconfirmed,
            ContinuationCause::SubmissionInterrupted.as_str(),
        )),
        (MutationStage::Settled, Some(MutationOutcome::Dispatched)) => {
            Some((ScheduleSettlement::Delivered, "delivered"))
        }
        (MutationStage::Settled, Some(MutationOutcome::ModelChangedBeforeAttempt)) => Some((
            ScheduleSettlement::Refused,
            ContinuationCause::ModelChanged.as_str(),
        )),
        (MutationStage::Settled, Some(MutationOutcome::AgentChangedBeforeAttempt)) => Some((
            ScheduleSettlement::Refused,
            ContinuationCause::AgentChanged.as_str(),
        )),
        (MutationStage::Settled, Some(MutationOutcome::RefusedBeforeAttempt)) => Some((
            ScheduleSettlement::Refused,
            ContinuationCause::PreferenceDisabled.as_str(),
        )),
        (
            MutationStage::Settled,
            Some(MutationOutcome::NoEffect | MutationOutcome::RefusedAfterSave),
        ) => Some((ScheduleSettlement::Refused, "revoked")),
        (MutationStage::Settled, Some(MutationOutcome::UnavailableAfterSave)) => {
            Some((ScheduleSettlement::Refused, "unavailable"))
        }
        _ => None,
    }
}

/// What one claimed row asked the delivery loop to do.
enum DeliveredStep {
    /// A retry is still ahead. The row is not due, and the work is not
    /// entered again until [`next_attempt`]'s time.
    Held,
    /// The row was submitted, cancelled, or left for `next_due`.
    Quiet,
}

/// The next retry after `previous` failures.
///
/// Submit, an outcome write, reconciliation, and a delivery whose plan did
/// not commit all use this. The first failure waits [`SCHEDULE_RETRY_MS`].
/// Each later failure doubles that wait, up to [`CATCH_UP_RETRY_MAX_MS`].
fn next_attempt(now_ms: u64, previous: u32) -> (u32, u64) {
    let failures = previous.saturating_add(1);
    let retry_at = now_ms.saturating_add(attempt_backoff(failures));
    (failures, retry_at)
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

fn sooner_hold(soonest: Option<u64>, retry_at: u64, now: u64) -> Option<u64> {
    if retry_at <= now {
        return soonest;
    }
    let wait = retry_at - now;
    Some(soonest.map_or(wait, |current| current.min(wait)))
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

fn scheduled_submit_binding(
    id: &ConversationId,
    row: &ScheduledMessage,
) -> Result<MutationBinding, ConversationError> {
    if let Some(evidence) = &row.command_evidence {
        return evidence.binding(id, &row.execution_id);
    }
    ConversationService::submit_lookup(
        id,
        &schedule_caller(row)?,
        &row.execution_id,
        &super::SubmittedMessage {
            text: row.prompt.clone(),
            ..Default::default()
        },
        super::SubmissionMode::Queue,
    )
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
#[path = "../../../../tests/conversation/scheduled_messages.rs"]
mod tests;
