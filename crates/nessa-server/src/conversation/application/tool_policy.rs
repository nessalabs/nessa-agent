//! The question a turn's tool policy answers: may the person this turn runs
//! for call this tool? (Slice J4 of #707, gate 18.)
//!
//! The owner's turns and a Drive grant's turns ask the same port. The port
//! is answered by Cedar (`infrastructure::tool_policy::cedar`); nothing here
//! compares names, presets or patterns. A decision names the policies that
//! made it, so a refusal can be audited with its rule.
//!
//! ```text
//! caller ──(initiator, call)──▶ ToolPolicyEvaluator ──▶ Allow { rules }
//!                                                    └─▶ Refuse { rules }
//!                                                    └─▶ ToolPolicyUnavailable
//! ```
//!
//! `Allow` is not an approval: it leaves the call to the conversation's
//! approval mode, so a person is still asked where that mode asks. A caller
//! treats `ToolPolicyUnavailable` as a refusal; the port never stands in for
//! an answer it could not give.

use crate::conversation::domain::{ToolCall, ToolPolicy};

/// Who a tool call runs for: the turn's initiator, with the policy it was
/// admitted under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolInitiator<'a> {
    /// The conversation's owner.
    Owner,
    /// A Drive grant, with the policy at the revision its turn was admitted
    /// under.
    Grant {
        /// The grant's identity text.
        grant: &'a str,
        /// The grant's tool policy at that revision.
        policy: &'a ToolPolicy,
    },
}

/// What a tool policy said about a call, with the ids of the policies that
/// said it, in order. A refusal with no ids is one no policy allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolDecision {
    /// No policy forbids the call and one permits it.
    Allow {
        /// The ids of the policies that permitted it.
        rules: Vec<String>,
    },
    /// A policy forbids the call, or none permits it.
    Refuse {
        /// The ids of the policies that forbade it; empty when none permitted it.
        rules: Vec<String>,
    },
}

/// The policy could not be evaluated: its bundle, the request or a grant's
/// rules did not validate, or evaluation reported an error. The caller
/// refuses the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolPolicyUnavailable;

/// Answers whether a turn's initiator may call a tool.
pub trait ToolPolicyEvaluator: Send + Sync {
    /// The decision for `call`, made for `initiator`.
    fn evaluate(
        &self,
        initiator: ToolInitiator<'_>,
        call: ToolCall<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable>;
}
