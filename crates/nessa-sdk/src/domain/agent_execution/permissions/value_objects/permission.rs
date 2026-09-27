#![deny(missing_docs)]

use super::{PermissionApplicationId, PermissionOptionId, PermissionSessionId};
use crate::domain::agent_execution::ExecutionError;
use std::collections::HashSet;

/// Whether the selected action is allowed. Scope is a separate value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermissionEffect {
    /// Permit the exact reviewed action under the chosen scope.
    Allow,
    /// Refuse the exact reviewed action under the chosen scope.
    Deny,
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
        self.0.iter().fold(0usize, |total, option| {
            total
                .saturating_add(std::mem::size_of::<PermissionOption>())
                .saturating_add(option.id().as_str().len())
                .saturating_add(option.label().len())
                .saturating_add(option.decision().payload_bytes())
        })
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
    /// Own validated custom `reason` without cancelling or mutating any review.
    pub fn custom(reason: CustomPermissionCancellationReason) -> Self {
        Self(PermissionCancellationReasonValue::Custom(reason))
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
            PermissionCancellationReasonValue::Custom(reason) => {
                PermissionCancellationReasonView::Custom(reason)
            }
        }
    }
}

/// A caller-defined cause, such as an automatic guard's decision.
/// Code identifies the cause consistently; explanation describes this cancellation.
/// Both preserve supplied text and must be nonblank. Bounds are UTF-8 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomPermissionCancellationReason {
    code: Box<str>,
    explanation: Box<str>,
}
impl CustomPermissionCancellationReason {
    /// Maximum UTF-8 byte length of the stable cause code.
    pub const MAX_CODE_BYTES: usize = 128;
    /// Maximum UTF-8 byte length of the descriptive explanation.
    pub const MAX_EXPLANATION_BYTES: usize = 1024;

    /// Preserve `code` and `explanation` exactly. Returns [`ExecutionError::EmptyValue`]
    /// if either is blank or [`ExecutionError::ValueTooLong`] if its UTF-8 bytes
    /// exceed the corresponding maximum. Construction performs no cancellation.
    pub fn new(
        code: impl Into<String>,
        explanation: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        let reason = Self {
            code: code.into().into_boxed_str(),
            explanation: explanation.into().into_boxed_str(),
        };
        reason.validate()?;
        Ok(reason)
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
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/permission_policy_storage.rs"]
mod storage_tests;
