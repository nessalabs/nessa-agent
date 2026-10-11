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

/// The refusal outcome configured for one binding.
///
/// [`Continue`](Self::Continue) carries no rule: a built-in refusal still means
/// the agent is told no and the execution continues. [`Stop`](Self::Stop) is an
/// identified rule, and its kind cannot be [`RefusalOutcome::Continue`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfiguredRefusal {
    /// Tell the agent no and leave the execution running.
    Continue,
    /// Stop for `rule`. `kind` ends the turn or closes the session.
    Stop {
        /// Whether the stop ends the turn or closes the session.
        kind: RefusalOutcome,
        /// The rule that required the stop.
        rule: ApprovalRuleReference,
    },
}

impl ConfiguredRefusal {
    /// Tell the agent no and leave the execution running. No rule is involved.
    pub fn continue_turn() -> Self {
        Self::Continue
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
        Ok(Self::Stop {
            kind: outcome,
            rule,
        })
    }

    /// What this binding does when it refuses a tool review.
    pub fn outcome(&self) -> RefusalOutcome {
        match self {
            Self::Continue => RefusalOutcome::Continue,
            Self::Stop { kind, .. } => *kind,
        }
    }

    /// The rule that required a stop, when this outcome is not continue.
    pub fn rule(&self) -> Option<&ApprovalRuleReference> {
        match self {
            Self::Continue => None,
            Self::Stop { rule, .. } => Some(rule),
        }
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
    match configured {
        ConfiguredRefusal::Continue => Ok(()),
        ConfiguredRefusal::Stop {
            kind: RefusalOutcome::Continue,
            ..
        } => Err(AgentError::Configuration(
            "a stopping refusal cannot continue the turn".into(),
        )),
        ConfiguredRefusal::Stop {
            kind: RefusalOutcome::EndTurn,
            ..
        } => match capabilities.policy_end_turn() {
            PolicyEndTurnCapability::SupportedForCurrentInvocation => Ok(()),
            PolicyEndTurnCapability::Unknown => Err(AgentError::Configuration(
                "configured refusal ends the turn, and this binding has not proven it can stop the current invocation and run another".into(),
            )),
            PolicyEndTurnCapability::Unsupported => Err(AgentError::Configuration(
                "configured refusal ends the turn, and this binding cannot stop the current invocation and run another".into(),
            )),
        },
        ConfiguredRefusal::Stop {
            kind: RefusalOutcome::CloseSession,
            ..
        } => match capabilities.policy_close_session() {
            PolicyCloseSessionCapability::SupportedForSession => Ok(()),
            PolicyCloseSessionCapability::Unknown => Err(AgentError::Configuration(
                "configured refusal closes the session, and this binding has not proven it owns that close".into(),
            )),
            PolicyCloseSessionCapability::Unsupported => Err(AgentError::Configuration(
                "configured refusal closes the session, and this binding cannot close it".into(),
            )),
        },
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/permissions/outcome.rs"]
mod tests;
