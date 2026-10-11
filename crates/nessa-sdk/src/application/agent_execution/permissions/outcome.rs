//! Which refusal outcome a binding is allowed to carry.
//!
//! Adapters declare turn-cleanup and session-close facts.
//! [`OperationCapabilities::resolve`](crate::application::agent_execution::providers::OperationCapabilities::resolve)
//! is the only place those facts become a capability. This module asks that
//! capability whether a configured outcome can be honored. It does not invent
//! a second answer, and it never promotes ending a turn into closing the session.
#![deny(missing_docs)]

use super::ApprovalRuleReference;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::providers::{
    OperationCapabilities, PolicyCloseSessionCapability, PolicyEndTurnCapability,
};
use crate::domain::agent_execution::permissions::RefusalOutcome;

/// The refusal outcome configured for one binding, with the rule that required a stop.
///
/// [`continue_turn`](Self::continue_turn) carries no rule: a built-in refusal
/// still means the agent is told no and the execution continues.
/// [`stopping`](Self::stopping) is an identified rule, and it cannot name
/// [`RefusalOutcome::Continue`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfiguredRefusal {
    outcome: RefusalOutcome,
    rule: Option<ApprovalRuleReference>,
}

impl ConfiguredRefusal {
    /// Tell the agent no and leave the execution running. No rule is involved.
    pub fn continue_turn() -> Self {
        Self {
            outcome: RefusalOutcome::Continue,
            rule: None,
        }
    }

    /// Stop for `rule`. `outcome` must end the turn or close the session.
    ///
    /// Returns [`AgentError::InvalidInput`] when `outcome` is
    /// [`RefusalOutcome::Continue`], because a stop without a stop is not a
    /// configured outcome.
    pub fn stopping(
        outcome: RefusalOutcome,
        rule: ApprovalRuleReference,
    ) -> Result<Self, AgentError> {
        if outcome == RefusalOutcome::Continue {
            return Err(AgentError::InvalidInput(
                "a stopping refusal cannot continue the turn".into(),
            ));
        }
        Ok(Self {
            outcome,
            rule: Some(rule),
        })
    }

    /// What this binding does when it refuses a tool review.
    pub fn outcome(&self) -> RefusalOutcome {
        self.outcome
    }

    /// The rule that required a stop, when this outcome is not continue.
    pub fn rule(&self) -> Option<&ApprovalRuleReference> {
        self.rule.as_ref()
    }
}

/// Admit `configured` against the capabilities this binding has already resolved.
///
/// Continue is always admitted. Ending the turn is admitted only when the
/// binding can stop the current invocation and run another. Closing the session
/// is admitted only when the binding owns that close. An end-turn policy is
/// refused when that cleanup is unknown or unsupported. It is not rewritten
/// into a session close.
///
/// # Errors
///
/// Returns [`AgentError::Configuration`] naming the outcome and the missing
/// proof. The returned error is the activation refusal: the binding must not
/// become usable.
pub fn admit_configured_outcome(
    configured: &ConfiguredRefusal,
    capabilities: OperationCapabilities,
) -> Result<(), AgentError> {
    match configured.outcome() {
        RefusalOutcome::Continue => Ok(()),
        RefusalOutcome::EndTurn => match capabilities.policy_end_turn() {
            PolicyEndTurnCapability::SupportedForCurrentInvocation => Ok(()),
            PolicyEndTurnCapability::Unknown => Err(AgentError::Configuration(
                "configured refusal ends the turn, and this binding has not proven it can stop the current invocation and run another".into(),
            )),
            PolicyEndTurnCapability::UnsupportedNotImplemented => Err(AgentError::Configuration(
                "configured refusal ends the turn, and this binding cannot stop the current invocation and run another".into(),
            )),
        },
        RefusalOutcome::CloseSession => match capabilities.policy_close_session() {
            PolicyCloseSessionCapability::SupportedForSession => Ok(()),
            PolicyCloseSessionCapability::Unknown => Err(AgentError::Configuration(
                "configured refusal closes the session, and this binding has not proven it owns that close".into(),
            )),
            PolicyCloseSessionCapability::UnsupportedNotImplemented => Err(AgentError::Configuration(
                "configured refusal closes the session, and this binding cannot close it".into(),
            )),
        },
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/permissions/outcome.rs"]
mod tests;
