//! The question a turn's command policy answers: may this turn's initiator
//! use this host, and run this program on it? (ADR 803 step 4.)
//!
//! Owner, agent-authored and Drive grant turns ask the same port. It is
//! answered by Cedar (`infrastructure::tool_policy::cedar`), from what is
//! true: the hosts and program lists this gateway is configured with, the
//! hosts a grant's policy names for itself (`ToolPolicy::command_hosts`), and
//! the program as asked. Nothing here compares hosts or program names.
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
//! `run` asks [`admit_command`] and `environments_list` asks
//! [`CommandQuestion::Host`] for the running turn's initiator: the owner, an
//! agent, or the Drive grant that started it with its policy at the revision
//! the turn was admitted under (`AdmittedPolicies`). Cedar refuses commands
//! for agent-authored turns. Whether a grant's turn may call `run` at all is
//! its tool policy's (`drive_tools`), asked first.

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
    /// An agent-authored turn, distinct from the conversation's owner.
    Agent,
    /// A Drive grant.
    Grant {
        /// The grant's identity text.
        grant: &'a str,
        /// The grant's policy at the revision its turn was admitted under,
        /// whose command hosts are the ones it may use: none when it names
        /// none.
        policy: &'a ToolPolicy,
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
