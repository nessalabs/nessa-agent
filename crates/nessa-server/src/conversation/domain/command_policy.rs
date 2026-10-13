//! Which commands a conversation's agent may run, and where (issue #700):
//! the hosts this gateway grants commands on, and the programs it allows.
//!
//! ```text
//! admit(host, program) ──▶ host granted? ── no ──▶ EnvironmentNotGranted
//!                          program denied, or not allowed? ── yes ──▶ CommandDenied
//!                          ──▶ admitted
//! ```
//!
//! Arrows are checks, in order. A program is matched by its file name, the
//! part after its last `/`, so `/usr/bin/rm` is `rm`. A denial wins over an
//! allowance, and with an allow-list only a bare name is allowed. This is the gateway's tool policy, not a sandbox: an allowed
//! program can still run any other it is given as an argument.
//!
//! The policy is also read as facts by the command policy's Cedar adapter
//! (`infrastructure::tool_policy::cedar`, ADR 803 step 4): its hosts and
//! program lists, and [`program_name`]. [`CommandPolicy::admit`] and
//! [`CommandPolicy::grants`] are the Rust answer `run` and `environments_list`
//! still ask today; they are deleted once both ask `CommandPolicyEvaluator`
//! (ADR 803 step 4, after slice J4 part 3), which also limits a Drive grant's
//! turn to its own hosts.
use nessa_sdk::domain::agent_execution::leases::CommandRefusal;
use std::collections::BTreeSet;

/// The hosts commands are granted on, and the programs allowed there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandPolicy {
    hosts: BTreeSet<String>,
    allow: Option<BTreeSet<String>>,
    deny: BTreeSet<String>,
}

impl CommandPolicy {
    /// Commands granted on `hosts`; of those, only the programs in `allow`
    /// when it is given, and never one in `deny`. Each program is named by
    /// its file name.
    pub fn new(
        hosts: impl IntoIterator<Item = String>,
        allow: Option<Vec<String>>,
        deny: Vec<String>,
    ) -> Self {
        Self {
            hosts: hosts.into_iter().collect(),
            allow: allow.map(|allow| allow.into_iter().collect()),
            deny: deny.into_iter().collect(),
        }
    }

    /// The hosts commands are granted on.
    pub fn hosts(&self) -> &BTreeSet<String> {
        &self.hosts
    }

    /// The programs allowed by file name; `None` when there is no allow-list
    /// and every program not denied is allowed.
    pub fn allow(&self) -> Option<&BTreeSet<String>> {
        self.allow.as_ref()
    }

    /// The programs denied by file name.
    pub fn deny(&self) -> &BTreeSet<String> {
        &self.deny
    }

    /// Whether commands are granted on `host` at all. Replaced by
    /// `CommandPolicyEvaluator`'s `Host` question once `run` asks it.
    pub fn grants(&self, host: &str) -> bool {
        self.hosts.contains(host)
    }

    /// The policy as lines, each one fact, in a fixed order: what a digest of
    /// it is taken over, so two policies read alike only when they are.
    pub fn describe(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .hosts
            .iter()
            .map(|host| format!("host {host}"))
            .collect();
        match &self.allow {
            Some(allow) => lines.extend(allow.iter().map(|program| format!("allow {program}"))),
            None => lines.push("allow *".into()),
        }
        lines.extend(self.deny.iter().map(|program| format!("deny {program}")));
        lines
    }

    /// Whether `program` may run on `host`, or the refusal it meets.
    /// Replaced by `admit_command` over `CommandPolicyEvaluator` once `run`
    /// asks it.
    pub fn admit(&self, host: &str, program: &str) -> Result<(), CommandRefusal> {
        if !self.grants(host) {
            return Err(CommandRefusal::EnvironmentNotGranted);
        }
        let name = program_name(program);
        // With an allow-list, a program is named bare, and the host finds
        // it only in absolute search directories outside its workspace, so
        // a file the agent wrote under an allowed name is not run.
        let allowed = self
            .allow
            .as_ref()
            .is_none_or(|allow| name == program && allow.contains(name));
        if !allowed || self.deny.contains(name) {
            return Err(CommandRefusal::CommandDenied);
        }
        Ok(())
    }
}

/// A program's file name: what follows its last `/`, so `/usr/bin/rm` is
/// `rm`. A fact the command policy is read with, not a decision.
pub fn program_name(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

#[cfg(test)]
#[path = "../../../tests/conversation/command_policy.rs"]
mod tests;
