//! The question a turn's command policy answers: may the person this turn
//! runs for use this host, and run this program on it? (ADR 803 step 4.)
//!
//! The owner's turns and a Drive grant's turns ask the same port. It is
//! answered by Cedar (`infrastructure::tool_policy::cedar`), from what is
//! true: the hosts and program lists this gateway is configured with, the
//! hosts a grant names for itself, and the program as asked. Nothing here
//! compares hosts or program names.
//!
//! ```text
//! caller ──(initiator, Host{host})──────────▶ CommandPolicyEvaluator ──▶ Allow { rules }
//! caller ──(initiator, Run{host, program})──▶                        └─▶ Refuse { rules }
//!                                                                    └─▶ ToolPolicyUnavailable
//! ```
//!
//! A grant that names no hosts uses none, whatever the gateway is configured
//! with: a Drive turn does not inherit the owner's hosts.
//!
//! Not wired yet: `run` still asks `CommandPolicy::admit` and
//! `environments_list` still asks `CommandPolicy::grants`. Both move to
//! [`admit_command`] and [`CommandQuestion::Host`] in the same change, once
//! slice J4 part 3 gives `run` the turn's initiation (ADR 803 step 4).

use std::collections::BTreeSet;

use nessa_sdk::domain::agent_execution::leases::CommandRefusal;

use super::{ToolDecision, ToolPolicyUnavailable};
use crate::conversation::domain::ToolPolicy;

/// What is asked of the command policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandQuestion<'a> {
    /// May the initiator run commands on `host` at all?
    Host {
        /// The host, as `run` names it: an SSH destination or a `peer:` name.
        host: &'a str,
    },
    /// May the initiator run `program` on `host`? Allowed only where the
    /// `Host` question is too.
    Run {
        /// The host, as for `Host`.
        host: &'a str,
        /// The program as the agent named it, bare or a path.
        program: &'a str,
    },
}

/// Who a command runs for: the turn's initiator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandInitiator<'a> {
    /// The conversation's owner.
    Owner,
    /// A Drive grant.
    Grant {
        /// The grant's identity text.
        grant: &'a str,
        /// The grant's tool policy at the revision its turn was admitted
        /// under: one fact of the same grant the tool policy reads.
        policy: &'a ToolPolicy,
        /// The hosts the grant names for itself; `None` when it names none,
        /// and so may use none.
        hosts: Option<&'a BTreeSet<String>>,
    },
}

/// Answers whether a turn's initiator may use a host, or run a program on it.
pub trait CommandPolicyEvaluator: Send + Sync {
    /// The decision for `question`, made for `initiator`. The policy ids in
    /// a refusal name the rule that forbade it; none means no rule allowed it.
    fn evaluate(
        &self,
        initiator: CommandInitiator<'_>,
        question: CommandQuestion<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable>;
}

/// Whether `initiator` may run `program` on `host`, as `run` asks it: the
/// host first, so a host not granted is refused as such before its program
/// is looked at. `Ok` carries the ids of the policies that allowed it.
/// A policy that could not be evaluated refuses, as the refusal it would
/// have met at that question.
pub fn admit_command(
    evaluator: &dyn CommandPolicyEvaluator,
    initiator: CommandInitiator<'_>,
    host: &str,
    program: &str,
) -> Result<Vec<String>, CommandRefusal> {
    match evaluator.evaluate(initiator, CommandQuestion::Host { host }) {
        Ok(ToolDecision::Allow { .. }) => {}
        Ok(ToolDecision::Refuse { rules }) => {
            tracing::info!(host, ?rules, "command host refused by policy");
            return Err(CommandRefusal::EnvironmentNotGranted);
        }
        Err(ToolPolicyUnavailable) => {
            tracing::error!(host, "command policy unavailable; host refused");
            return Err(CommandRefusal::EnvironmentNotGranted);
        }
    }
    match evaluator.evaluate(initiator, CommandQuestion::Run { host, program }) {
        Ok(ToolDecision::Allow { rules }) => Ok(rules),
        Ok(ToolDecision::Refuse { rules }) => {
            tracing::info!(host, program, ?rules, "command refused by policy");
            Err(CommandRefusal::CommandDenied)
        }
        Err(ToolPolicyUnavailable) => {
            tracing::error!(host, program, "command policy unavailable; command refused");
            Err(CommandRefusal::CommandDenied)
        }
    }
}
