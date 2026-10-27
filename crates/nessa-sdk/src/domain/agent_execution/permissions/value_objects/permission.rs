#![deny(missing_docs)]

use super::{
    PermissionApplicationId, PermissionId, PermissionOptionId, PermissionSessionId, RefusalOutcome,
};
use crate::domain::agent_execution::{
    executions::ExecutionId, sessions::ExecutionSessionId, ExecutionError,
};
use std::collections::HashSet;

/// Whether the selected action is allowed. Scope is a separate value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermissionEffect {
    /// Permit the exact reviewed action under the chosen scope.
    Allow,
    /// Refuse the exact reviewed action under the chosen scope.
    Deny,
}

impl PermissionEffect {
    /// `allow` or `deny`, the spelling audits and views record.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

impl From<PermissionEffect> for &'static str {
    fn from(effect: PermissionEffect) -> Self {
        effect.as_str()
    }
}

impl TryFrom<&str> for PermissionEffect {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            _ => Err(()),
        }
    }
}

/// Where a decision applies. Request scope means the exact owning PermissionRequest,
/// and its correlated tool. The application retains the review input.
/// Session IDs are qualified by their application boundary.
/// Broader scope does not widen the tool/resource target or imply durable storage.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PermissionScope(PermissionScopeValue);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PermissionScopeValue {
    Request,
    Session {
        application_id: PermissionApplicationId,
        session_id: PermissionSessionId,
    },
    Application(PermissionApplicationId),
}

/// Borrowed scope identities. Inspecting a scope never exposes mutable identity storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionScopeView<'a> {
    /// Only the owning review and correlated tool input.
    Request,
    /// A session-qualified decision; enforcement is an adapter capability.
    Session {
        /// Application boundary qualifying the session identity.
        application_id: &'a PermissionApplicationId,
        /// Session within that application boundary.
        session_id: &'a PermissionSessionId,
    },
    /// A decision within the named application; does not create a durable grant.
    Application(&'a PermissionApplicationId),
}

impl PermissionScope {
    /// Describe only the owning review and correlated tool input; grants no authority.
    pub fn request() -> Self {
        Self(PermissionScopeValue::Request)
    }
    /// Own validated `application_id` and `session_id` as one qualified scope.
    /// Construction neither approves an action nor persists a reusable grant.
    pub fn session(
        application_id: PermissionApplicationId,
        session_id: PermissionSessionId,
    ) -> Self {
        Self(PermissionScopeValue::Session {
            application_id,
            session_id,
        })
    }
    /// Own validated `application_id` as the scope boundary; creates no durable grant.
    pub fn application(application_id: PermissionApplicationId) -> Self {
        Self(PermissionScopeValue::Application(application_id))
    }
    /// Borrow the scope and its immutable identities. To change scope, construct a replacement.
    ///
    /// ```compile_fail
    /// use nessa_sdk::domain::agent_execution::permissions::{PermissionApplicationId, PermissionScope, PermissionScopeView};
    /// let mut scope = PermissionScope::application(PermissionApplicationId::new("app").unwrap());
    /// if let PermissionScopeView::Application(id) = scope.view() {
    ///     *id = PermissionApplicationId::new("other").unwrap();
    /// }
    /// ```
    pub fn view(&self) -> PermissionScopeView<'_> {
        match &self.0 {
            PermissionScopeValue::Request => PermissionScopeView::Request,
            PermissionScopeValue::Session {
                application_id,
                session_id,
            } => PermissionScopeView::Session {
                application_id,
                session_id,
            },
            PermissionScopeValue::Application(id) => PermissionScopeView::Application(id),
        }
    }
}

/// Immutable decision intent, not an authorization credential or a stored grant.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PermissionDecision {
    effect: PermissionEffect,
    scope: PermissionScope,
}
impl PermissionDecision {
    /// Combine `effect` and validated `scope` without granting authority or persisting it.
    pub fn new(effect: PermissionEffect, scope: PermissionScope) -> Self {
        Self { effect, scope }
    }
    /// Effect requested by this decision.
    pub fn effect(&self) -> PermissionEffect {
        self.effect
    }
    pub(crate) fn payload_bytes(&self) -> usize {
        match self.scope.view() {
            PermissionScopeView::Request => 0,
            PermissionScopeView::Session {
                application_id,
                session_id,
            } => application_id
                .as_str()
                .len()
                .saturating_add(session_id.as_str().len()),
            PermissionScopeView::Application(id) => id.as_str().len(),
        }
    }
    /// Exact scope intended for this decision.
    pub fn scope(&self) -> &PermissionScope {
        &self.scope
    }
}

/// Which exact effects and scopes the host permits a request to offer. Configuration does
/// not approve a tool, bypass policy, or persist a permission on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOfferPolicy {
    decisions: Box<[PermissionDecision]>,
}
impl PermissionOfferPolicy {
    /// Construct the allowed offer set from exact effect/scope decisions.
    /// Returns an error for an empty set or duplicate decisions. A borrowed set
    /// checks duplicates without pairwise scans. Retains configured order in
    /// compact immutable storage, discarding the input vector's spare capacity.
    pub fn new(decisions: Vec<PermissionDecision>) -> Result<Self, ExecutionError> {
        if decisions.is_empty() {
            return Err(ExecutionError::NoPermissionOptions);
        }
        let mut unique = HashSet::with_capacity(decisions.len());
        for decision in &decisions {
            if !unique.insert(decision) {
                return Err(ExecutionError::DuplicatePermissionDecision);
            }
        }
        Ok(Self {
            decisions: decisions.into_boxed_slice(),
        })
    }
    /// Permit allow and deny choices for this request only, with no reusable grant.
    pub fn once_only() -> Self {
        Self {
            decisions: vec![
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
                PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
            ]
            .into_boxed_slice(),
        }
    }
    /// Allowed choices in their configured order.
    pub fn decisions(&self) -> &[PermissionDecision] {
        &self.decisions
    }
    /// Whether this exact effect/scope may be offered; this does not authorize it.
    pub fn allows(&self, decision: &PermissionDecision) -> bool {
        self.decisions.contains(decision)
    }
}

/// A specific review choice. Distinct scopes can share the same effect;
/// answers select the option identity, never an ambiguous boolean or kind alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOption {
    id: PermissionOptionId,
    label: Box<str>,
    decision: PermissionDecision,
}
impl PermissionOption {
    /// Own `id`, the display `label`, and its exact `decision`. Returns
    /// [`ExecutionError::EmptyValue`] for a blank label; preserves accepted text.
    pub fn new(
        id: PermissionOptionId,
        label: impl Into<String>,
        decision: PermissionDecision,
    ) -> Result<Self, ExecutionError> {
        let label = label.into();
        if label.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("permission option label"));
        }
        Ok(Self {
            id,
            label: label.into_boxed_str(),
            decision,
        })
    }
    /// Stable choice identity used by an answer to select this exact option.
    pub fn id(&self) -> &PermissionOptionId {
        &self.id
    }
    /// Original human-readable option label.
    pub fn label(&self) -> &str {
        &self.label
    }
    /// Effect and scope offered by this choice.
    pub fn decision(&self) -> &PermissionDecision {
        &self.decision
    }
}

/// Nonempty, uniquely identified choices admitted by host configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOptions(Box<[PermissionOption]>);
impl PermissionOptions {
    /// Filter owned `options` through the host's allowed `config`, preserving order.
    /// Borrowed sets check identity uniqueness and allowed decisions without
    /// pairwise scans or repeated full-policy scans.
    /// Duplicate identities are rejected before filtering, including disallowed
    /// choices. Returns [`ExecutionError::DuplicatePermissionOption`] for a repeated
    /// ID or [`ExecutionError::NoPermissionOptions`] if no allowed choices remain.
    pub fn new(
        options: Vec<PermissionOption>,
        config: &PermissionOfferPolicy,
    ) -> Result<Self, ExecutionError> {
        let mut identities = HashSet::with_capacity(options.len());
        for option in &options {
            if !identities.insert(option.id()) {
                return Err(ExecutionError::DuplicatePermissionOption);
            }
        }
        let allowed: HashSet<_> = config.decisions().iter().collect();
        let options: Vec<_> = options
            .into_iter()
            .filter(|option| allowed.contains(option.decision()))
            .collect();
        if options.is_empty() {
            return Err(ExecutionError::NoPermissionOptions);
        }
        Ok(Self(options.into_boxed_slice()))
    }
    /// Borrow admitted choices in their original order.
    pub fn choices(&self) -> &[PermissionOption] {
        &self.0
    }
    /// Retained choice storage in bytes, including option slots, labels, identities,
    /// and scoped identities. Immutable storage has no spare string/vector capacity.
    /// Callers choose their admission budget; this measurement imposes no policy cap.
    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes_up_to(usize::MAX)
    }
    /// [`Self::payload_bytes`], stopping once the count passes `limit`. A
    /// result above `limit` says only that the choices are larger than `limit`,
    /// so sizing a long list does not walk every choice.
    pub fn payload_bytes_up_to(&self, limit: usize) -> usize {
        let mut total = 0usize;
        for option in self.0.iter() {
            if total > limit {
                return total;
            }
            total = total
                .saturating_add(std::mem::size_of::<PermissionOption>())
                .saturating_add(option.id().as_str().len())
                .saturating_add(option.label().len())
                .saturating_add(option.decision().payload_bytes());
        }
        total
    }
    /// Find the admitted option with exact `id`, or None if absent.
    pub fn find(&self, id: &PermissionOptionId) -> Option<&PermissionOption> {
        self.0.iter().find(|option| option.id() == id)
    }
}

/// Why a pending review ended without a permission decision.
/// The first cancellation retains its cause even when later cleanup has another cause.
/// Construct a replacement to change a cause; its custom payload is never mutable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionCancellationReason(PermissionCancellationReasonValue);

#[derive(Clone, Debug, PartialEq, Eq)]
enum PermissionCancellationReasonValue {
    /// The provider withdrew its pending review.
    ProviderWithdrawal,
    /// The session was explicitly closed.
    SessionClosed,
    /// The provider attachment failed, whether idle or executing.
    SessionFailed,
    /// Execution completed while the review remained pending.
    ExecutionFinished,
    /// Execution failed while the review remained pending.
    ExecutionFailed,
    /// A bounded review, execution, startup, or session-control operation expired.
    DeadlineExceeded,
    /// The required provider observation reader was dropped.
    EventConsumerDropped,
    /// All controlling session handles were dropped.
    SessionHandlesDropped,
    /// A configured refusal closed the session.
    PolicyRefusal,
    /// A validated custom lifecycle reason, retained exactly.
    Custom(CustomPermissionCancellationReason),
}

/// Borrowed cancellation cause, including immutable custom code and explanation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionCancellationReasonView<'a> {
    /// The provider withdrew its pending review.
    ProviderWithdrawal,
    /// The session was explicitly closed.
    SessionClosed,
    /// The provider attachment failed, whether idle or executing.
    SessionFailed,
    /// Execution completed while the review remained pending.
    ExecutionFinished,
    /// Execution failed while the review remained pending.
    ExecutionFailed,
    /// A bounded review, execution, startup, or session-control operation expired.
    DeadlineExceeded,
    /// The required provider observation reader was dropped.
    EventConsumerDropped,
    /// All controlling session handles were dropped.
    SessionHandlesDropped,
    /// A configured refusal closed the session.
    PolicyRefusal,
    /// A validated custom lifecycle reason, retained exactly.
    Custom(&'a CustomPermissionCancellationReason),
}

impl PermissionCancellationReason {
    /// The provider withdrew its pending review. This value performs no cancellation.
    pub fn provider_withdrawal() -> Self {
        Self(PermissionCancellationReasonValue::ProviderWithdrawal)
    }
    /// The session was explicitly closed. This value performs no cancellation.
    pub fn session_closed() -> Self {
        Self(PermissionCancellationReasonValue::SessionClosed)
    }
    /// The provider attachment failed, whether idle or executing. This value performs no cancellation.
    pub fn session_failed() -> Self {
        Self(PermissionCancellationReasonValue::SessionFailed)
    }
    /// Execution completed while the review remained pending. This value performs no cancellation.
    pub fn execution_finished() -> Self {
        Self(PermissionCancellationReasonValue::ExecutionFinished)
    }
    /// Execution failed while the review remained pending. This value performs no cancellation.
    pub fn execution_failed() -> Self {
        Self(PermissionCancellationReasonValue::ExecutionFailed)
    }
    /// A bounded review, execution, startup, or session-control operation expired. This value performs no cancellation.
    pub fn deadline_exceeded() -> Self {
        Self(PermissionCancellationReasonValue::DeadlineExceeded)
    }
    /// The required provider observation reader was dropped. This value performs no cancellation.
    pub fn event_consumer_dropped() -> Self {
        Self(PermissionCancellationReasonValue::EventConsumerDropped)
    }
    /// All controlling session handles were dropped. This value performs no cancellation.
    pub fn session_handles_dropped() -> Self {
        Self(PermissionCancellationReasonValue::SessionHandlesDropped)
    }
    /// A configured refusal closed the session. This value performs no cancellation.
    pub fn policy_refusal() -> Self {
        Self(PermissionCancellationReasonValue::PolicyRefusal)
    }
    /// Own validated custom `reason` without cancelling or mutating any review.
    pub fn custom(reason: CustomPermissionCancellationReason) -> Self {
        Self(PermissionCancellationReasonValue::Custom(reason))
    }
    /// Whether this reason may withdraw `review` in `execution` of `session`: a
    /// hook-refusal code withdraws only the review, in the session and turn,
    /// the runner issued it for, and `None` allows none, so a copy of a
    /// recorded one, or a saved one, withdraws nothing.
    pub(crate) fn may_withdraw(
        &self,
        review: Option<(&ExecutionSessionId, &ExecutionId, &PermissionId)>,
    ) -> bool {
        match &self.0 {
            PermissionCancellationReasonValue::Custom(reason) if reason.is_hook_refusal() => review
                .is_some_and(|review| {
                    reason
                        .withdraws
                        .as_ref()
                        .is_some_and(|(session, execution, id)| (session, execution, id) == review)
                }),
            _ => true,
        }
    }
    /// Whether this is a custom reason with a hook-refusal code.
    pub(crate) fn is_hook_refusal(&self) -> bool {
        matches!(&self.0, PermissionCancellationReasonValue::Custom(reason) if reason.is_hook_refusal())
    }
    /// Borrow the cause and any validated custom payload without mutation authority.
    ///
    /// ```compile_fail
    /// use nessa_sdk::domain::agent_execution::permissions::{CustomPermissionCancellationReason, PermissionCancellationReason, PermissionCancellationReasonView};
    /// let reason = PermissionCancellationReason::custom(
    ///     CustomPermissionCancellationReason::new("guard", "review withdrawn").unwrap());
    /// if let PermissionCancellationReasonView::Custom(custom) = reason.view() {
    ///     *custom = CustomPermissionCancellationReason::new("other", "different cause").unwrap();
    /// }
    /// ```
    pub fn view(&self) -> PermissionCancellationReasonView<'_> {
        match &self.0 {
            PermissionCancellationReasonValue::ProviderWithdrawal => {
                PermissionCancellationReasonView::ProviderWithdrawal
            }
            PermissionCancellationReasonValue::SessionClosed => {
                PermissionCancellationReasonView::SessionClosed
            }
            PermissionCancellationReasonValue::SessionFailed => {
                PermissionCancellationReasonView::SessionFailed
            }
            PermissionCancellationReasonValue::ExecutionFinished => {
                PermissionCancellationReasonView::ExecutionFinished
            }
            PermissionCancellationReasonValue::ExecutionFailed => {
                PermissionCancellationReasonView::ExecutionFailed
            }
            PermissionCancellationReasonValue::DeadlineExceeded => {
                PermissionCancellationReasonView::DeadlineExceeded
            }
            PermissionCancellationReasonValue::EventConsumerDropped => {
                PermissionCancellationReasonView::EventConsumerDropped
            }
            PermissionCancellationReasonValue::SessionHandlesDropped => {
                PermissionCancellationReasonView::SessionHandlesDropped
            }
            PermissionCancellationReasonValue::PolicyRefusal => {
                PermissionCancellationReasonView::PolicyRefusal
            }
            PermissionCancellationReasonValue::Custom(reason) => {
                PermissionCancellationReasonView::Custom(reason)
            }
        }
    }
}

/// A caller-defined cause, such as an automatic guard's decision.
/// Code identifies the cause consistently; explanation describes this cancellation.
/// Both preserve supplied text and must be nonblank. Bounds are UTF-8 bytes.
/// Two reasons are equal when their code and explanation are.
#[derive(Clone, Debug)]
pub struct CustomPermissionCancellationReason {
    code: Box<str>,
    explanation: Box<str>,
    // The one review, in its session and turn, a hook-refusal code may
    // withdraw; never saved.
    withdraws: Option<(ExecutionSessionId, ExecutionId, PermissionId)>,
}
impl PartialEq for CustomPermissionCancellationReason {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code && self.explanation == other.explanation
    }
}
impl Eq for CustomPermissionCancellationReason {}
impl CustomPermissionCancellationReason {
    /// Maximum UTF-8 byte length of the stable cause code.
    pub const MAX_CODE_BYTES: usize = 128;
    /// Maximum UTF-8 byte length of the descriptive explanation.
    pub const MAX_EXPLANATION_BYTES: usize = 1024;
    /// Codes that record a permission-review hook's refusal. Only the runner
    /// that applied the refusal, and so ended the turn or closed the session
    /// it records, writes one.
    pub const HOOK_REFUSAL_PREFIX: &'static str = "hook-refusal.";

    /// Preserve `code` and `explanation` exactly. Returns [`ExecutionError::EmptyValue`]
    /// if either is blank or [`ExecutionError::ValueTooLong`] if its UTF-8 bytes
    /// exceed the corresponding maximum. A code that starts with
    /// [`Self::HOOK_REFUSAL_PREFIX`] is [`ExecutionError::ReservedCancellationCode`].
    /// Construction performs no cancellation.
    pub fn new(
        code: impl Into<String>,
        explanation: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        let reason = Self::restore(code, explanation)?;
        if reason.is_hook_refusal() {
            return Err(ExecutionError::ReservedCancellationCode);
        }
        Ok(reason)
    }
    fn is_hook_refusal(&self) -> bool {
        self.code.starts_with(Self::HOOK_REFUSAL_PREFIX)
    }
    /// The withdrawal the runner records for a hook refusal of `review` in
    /// `execution` of `session` with `outcome`. It can withdraw only that review.
    pub(crate) fn hook_refusal(
        session: &ExecutionSessionId,
        execution: &ExecutionId,
        review: &PermissionId,
        outcome: RefusalOutcome,
        explanation: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        let name = Self::outcome_name(&outcome);
        let mut reason =
            Self::restore(format!("{}{name}", Self::HOOK_REFUSAL_PREFIX), explanation)?;
        reason.withdraws = Some((session.clone(), execution.clone(), review.clone()));
        Ok(reason)
    }
    /// Rebuild a saved reason, checking its bounds. A code in the reserved
    /// namespace must be one the runner writes, or it is
    /// [`ExecutionError::ReservedCancellationCode`].
    pub(crate) fn restore(
        code: impl Into<String>,
        explanation: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        let reason = Self {
            code: code.into().into_boxed_str(),
            explanation: explanation.into().into_boxed_str(),
            withdraws: None,
        };
        reason.validate()?;
        if reason.is_hook_refusal()
            && ![
                RefusalOutcome::Continue,
                RefusalOutcome::EndTurn,
                RefusalOutcome::CloseSession,
            ]
            .iter()
            .any(|outcome| {
                reason.code[Self::HOOK_REFUSAL_PREFIX.len()..] == *Self::outcome_name(outcome)
            })
        {
            return Err(ExecutionError::ReservedCancellationCode);
        }
        Ok(reason)
    }
    fn outcome_name(outcome: &RefusalOutcome) -> &'static str {
        match outcome {
            RefusalOutcome::Continue => "continue",
            RefusalOutcome::EndTurn => "end_turn",
            RefusalOutcome::CloseSession => "close_session",
        }
    }
    fn validate(&self) -> Result<(), ExecutionError> {
        for (value, field, max_bytes) in [
            (&self.code, "cancellation reason code", Self::MAX_CODE_BYTES),
            (
                &self.explanation,
                "cancellation reason explanation",
                Self::MAX_EXPLANATION_BYTES,
            ),
        ] {
            if value.trim().is_empty() {
                return Err(ExecutionError::EmptyValue(field));
            }
            if value.len() > max_bytes {
                return Err(ExecutionError::ValueTooLong { field, max_bytes });
            }
        }
        Ok(())
    }
    /// Stable caller-defined cause code.
    pub fn code(&self) -> &str {
        &self.code
    }
    /// Human-readable explanation for this specific cancellation.
    pub fn explanation(&self) -> &str {
        &self.explanation
    }

    /// The stop this hook refusal applies, when it ends the turn or closes
    /// the session. A continue refusal is `None`: the turn keeps running.
    pub fn hook_stop(&self) -> Option<RefusalOutcome> {
        let name = self.code.strip_prefix(Self::HOOK_REFUSAL_PREFIX)?;
        match name {
            "end_turn" => Some(RefusalOutcome::EndTurn),
            "close_session" => Some(RefusalOutcome::CloseSession),
            _ => None,
        }
    }

    /// The runner's hook withdrawal, for tests outside this crate.
    ///
    /// Construction does not withdraw a review. The live controller still
    /// requires the hook's own actor.
    #[cfg(feature = "test-support")]
    pub fn hook_refusal_for_test(
        session: &ExecutionSessionId,
        execution: &ExecutionId,
        review: &PermissionId,
        outcome: RefusalOutcome,
        explanation: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        Self::hook_refusal(session, execution, review, outcome, explanation)
    }
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/permission_policy_storage.rs"]
mod storage_tests;
