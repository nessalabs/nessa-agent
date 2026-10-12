use std::sync::Arc;

use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::{
    permissions::{PermissionId, PermissionRequest, PermissionStateView, RefusalOutcome},
    sessions::ExecutionSessionId,
};

// All retained actor, mode, rule, and revision text in this attribution contract
// uses one bound; provider/execution identifiers own their separate limits.
fn required(value: impl Into<String>, field: &str) -> Result<Box<str>, AgentError> {
    let value = value.into();
    if value.trim().is_empty() {
        return Err(AgentError::InvalidInput(format!("empty {field}")));
    }
    if value.len() > ActionContext::MAX_IDENTITY_BYTES {
        return Err(AgentError::InvalidInput(format!(
            "{field} exceeds {} UTF-8 bytes",
            ActionContext::MAX_IDENTITY_BYTES
        )));
    }
    // Compact immutable storage prevents caller-provided spare capacity from
    // multiplying across invocation, scheduling, and permission evidence clones.
    Ok(value.into_boxed_str())
}

/// Attribution supplied by the authorizing host, never inferred from provider text.
/// Construction validates structure, not identity or access. The host must verify
/// the principal and its allowed surface before calling the execution port.
/// Request identity is qualified by this actor and the resolved permission target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionContext {
    principal_id: Box<str>,
    surface_id: Box<str>,
    request_id: Box<str>,
}
impl ActionContext {
    /// Maximum UTF-8 byte length of each principal, surface, and request identity.
    pub const MAX_IDENTITY_BYTES: usize = 256;
    /// Retain verified principal, surface, and logical action request identities.
    /// Each parameter preserves its exact text and must contain non-whitespace
    /// content within [`Self::MAX_IDENTITY_BYTES`] UTF-8 bytes. Blank or oversized
    /// input returns [`AgentError::InvalidInput`] before persistence or dispatch;
    /// this does not authenticate the identities.
    pub fn new(
        principal_id: impl Into<String>,
        surface_id: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Result<Self, AgentError> {
        Ok(Self {
            principal_id: required(principal_id, "principal ID")?,
            surface_id: required(surface_id, "surface ID")?,
            request_id: required(request_id, "action request ID")?,
        })
    }
    /// Principal verified by the host, including an automatic guard’s own identity.
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }
    /// Authorized surface from which the action originated.
    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }
    /// Logical action identity used with actor and target to correlate retries.
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

/// The host's effective mode and immutable configuration revision at decision time.
/// This is attribution, not a mode evaluator or permission to bypass host policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalModeSnapshot {
    name: Box<str>,
    configuration_revision: Box<str>,
}
impl ApprovalModeSnapshot {
    /// Retain the mode name and immutable configuration revision evaluated by the
    /// host. Both parameters must be nonblank and at most [`ActionContext::MAX_IDENTITY_BYTES`]
    /// UTF-8 bytes. InvalidInput rejects either violation before evidence retention;
    /// construction performs no policy evaluation.
    pub fn new(
        name: impl Into<String>,
        configuration_revision: impl Into<String>,
    ) -> Result<Self, AgentError> {
        Ok(Self {
            name: required(name, "approval mode")?,
            configuration_revision: required(
                configuration_revision,
                "approval configuration revision",
            )?,
        })
    }
    /// Effective approval mode recorded at decision time.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Immutable configuration revision that determined this mode.
    pub fn configuration_revision(&self) -> &str {
        &self.configuration_revision
    }
}

/// Identifies the exact rule revision evaluated by the host and who approved it.
/// The host retains that immutable revision; this reference does not create a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRuleReference {
    rule_id: Box<str>,
    revision: Box<str>,
    granted_by: ActionContext,
}
impl ApprovalRuleReference {
    /// Reference the exact rule/revision and host-verified grant action. Rejects
    /// blank or oversized rule identifiers/revisions with InvalidInput. Both text
    /// parameters are limited to [`ActionContext::MAX_IDENTITY_BYTES`] UTF-8 bytes; `granted_by`
    /// already carries validated attribution. Construction does not grant access.
    pub fn new(
        rule_id: impl Into<String>,
        revision: impl Into<String>,
        granted_by: ActionContext,
    ) -> Result<Self, AgentError> {
        Ok(Self {
            rule_id: required(rule_id, "approval rule ID")?,
            revision: required(revision, "approval rule revision")?,
            granted_by,
        })
    }
    /// Rule evaluated by the host.
    pub fn rule_id(&self) -> &str {
        &self.rule_id
    }
    /// Immutable evaluated rule revision retained by the host.
    pub fn revision(&self) -> &str {
        &self.revision
    }
    /// Verified actor and request that granted the referenced rule.
    pub fn granted_by(&self) -> &ActionContext {
        &self.granted_by
    }
}

/// Why this answer was selected. Automatic decisions still have their own actor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalBasis {
    /// A verified actor explicitly selected the offered option.
    Explicit,
    /// The host’s effective mode selected the option.
    Mode(ApprovalModeSnapshot),
    /// An identified rule revision selected the option.
    Rule(ApprovalRuleReference),
    /// A permission-review hook selected the option.
    ///
    /// The actor is the hook (`permission-review-hook`), not the person who
    /// submitted the turn. The value keeps the registration index when a hook
    /// returned, the capped reason, and the outcome that was actually applied.
    /// A runtime refusal has no index.
    Hook(HookRefusal),
}

/// A permission-review hook's refusal, retained with the answer or withdrawal.
///
/// The reason is capped so a hook cannot put an unbounded string into the
/// audit record. Construction does not answer a review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookRefusal {
    index: Option<usize>,
    reason: Box<str>,
    outcome: RefusalOutcome,
}

impl HookRefusal {
    /// Maximum UTF-8 byte length of the retained hook reason.
    pub const MAX_REASON_BYTES: usize = 1024;

    /// Retain `index` and a capped `reason` for a refusal whose outcome is
    /// continue.
    ///
    /// `index` is the hook's zero-based registration position. `None` means the
    /// runtime refused before a registered hook returned. A blank reason becomes
    /// `"hook refused"`. A reason longer than [`Self::MAX_REASON_BYTES`] is cut
    /// on a character boundary. This does not answer a review.
    ///
    /// End-turn and close-session are recorded only by the runner that cancels
    /// the turn or closes the session, so a caller cannot build a refusal that
    /// claims either outcome:
    ///
    /// ```
    /// use nessa_sdk::application::agent_execution::permissions::HookRefusal;
    /// use nessa_sdk::domain::agent_execution::permissions::RefusalOutcome;
    /// assert_eq!(HookRefusal::new(Some(0), "no").outcome(), &RefusalOutcome::Continue);
    /// ```
    ///
    /// ```compile_fail
    /// use nessa_sdk::application::agent_execution::permissions::HookRefusal;
    /// use nessa_sdk::domain::agent_execution::permissions::RefusalOutcome;
    /// let _ = HookRefusal::new(Some(0), "no", RefusalOutcome::EndTurn);
    /// ```
    pub fn new(index: Option<usize>, reason: impl Into<String>) -> Self {
        Self::applied(index, reason, RefusalOutcome::Continue)
    }

    /// The runner's refusal, carrying the outcome it applies after recording it.
    pub(crate) fn applied(
        index: Option<usize>,
        reason: impl Into<String>,
        outcome: RefusalOutcome,
    ) -> Self {
        Self {
            index,
            reason: cap_hook_reason(reason.into()).into_boxed_str(),
            outcome,
        }
    }

    /// Zero-based position of the hook in the invocation's registration list.
    ///
    /// `None` means the runtime refused: the deadline elapsed, another hook
    /// thread was still running, or the blocking task failed. It is not hook 0.
    pub fn index(&self) -> Option<usize> {
        self.index
    }

    /// Why the hook refused, capped at [`Self::MAX_REASON_BYTES`].
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// The outcome that was applied. An end-turn the binding cannot perform
    /// is recorded as continue.
    pub fn outcome(&self) -> &RefusalOutcome {
        &self.outcome
    }
}

/// Trim `reason`, name a blank one, and cut it to
/// [`HookRefusal::MAX_REASON_BYTES`] on a character boundary. A reason that
/// already fits is returned without a copy.
pub(crate) fn cap_hook_reason(reason: String) -> String {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return "hook refused".to_owned();
    }
    if trimmed.len() == reason.len() && reason.len() <= HookRefusal::MAX_REASON_BYTES {
        return reason;
    }
    let mut end = trimmed.len().min(HookRefusal::MAX_REASON_BYTES);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_owned()
}

/// Who answered and why, required for allowances and denials alike.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalAttribution {
    actor: ActionContext,
    basis: ApprovalBasis,
}
impl ApprovalAttribution {
    /// Pair the verified answering actor with the host-evaluated decision basis.
    /// Values are retained unchanged; construction performs no authorization or I/O.
    pub fn new(actor: ActionContext, basis: ApprovalBasis) -> Self {
        Self { actor, basis }
    }
    /// Verified answering actor; automatic decisions use their own principal.
    pub fn actor(&self) -> &ActionContext {
        &self.actor
    }
    /// Exact explicit, mode, rule, or hook basis retained with the decision.
    pub fn basis(&self) -> &ApprovalBasis {
        &self.basis
    }

    /// The permission-review hook answering `permission` with `refusal`.
    ///
    /// The principal is `permission-review-hook`. The surface is the hook's
    /// registration index, or `runtime` when no hook returned. Neither is the
    /// person who submitted the turn. Construction does not answer the review.
    pub fn hook(permission: &PermissionId, refusal: HookRefusal) -> Result<Self, AgentError> {
        let surface = match refusal.index() {
            Some(index) => index.to_string(),
            None => "runtime".to_owned(),
        };
        Ok(Self::new(
            ActionContext::new("permission-review-hook", surface, permission.as_str())?,
            ApprovalBasis::Hook(refusal),
        ))
    }
}

/// Immutable result of a resolved permission: owning session and exact request/execution/tool/input,
/// offered choice, effect/scope, and attribution. Provider observations carry no
/// trusted authorship. Adapters submit selection and wire delivery evidence to
/// ExecutionAudit before reporting success. This result is not a retry receipt
/// or proof that the provider executed the tool. The audit adapter owns record
/// identities, observation times, and its documented durability contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionResolution {
    session_id: ExecutionSessionId,
    request: Arc<PermissionRequest>,
    input: ToolReviewInput,
    attribution: ApprovalAttribution,
}
impl PermissionResolution {
    pub(in crate::application::agent_execution) fn new(
        session_id: ExecutionSessionId,
        request: PermissionRequest,
        input: ToolReviewInput,
        attribution: ApprovalAttribution,
    ) -> Result<Self, AgentError> {
        if !matches!(request.state(), PermissionStateView::Answered { .. }) {
            return Err(AgentError::InvalidInput(
                "permission must be answered before attribution is recorded".into(),
            ));
        }
        Ok(Self {
            session_id,
            request: Arc::new(request),
            input,
            attribution,
        })
    }
    /// Provider context supplied by the controller that resolved this permission.
    /// Cloning or recording delivery preserves this ownership unchanged.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Read-only terminal request, including offered options and chosen decision.
    pub fn request(&self) -> &PermissionRequest {
        &self.request
    }
    /// Original tool input reviewed by the actor, never a later tool update.
    pub fn input(&self) -> &ToolReviewInput {
        &self.input
    }
    /// Host-verified actor and evaluated decision basis.
    pub fn attribution(&self) -> &ApprovalAttribution {
        &self.attribution
    }
}
