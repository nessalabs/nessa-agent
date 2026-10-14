//! Which commands a conversation's agent may run, and where (issue #700), as
//! this gateway is configured: the hosts it grants commands on, the programs
//! it allows, and the programs it denies.
//!
//! This is configuration, not a decision. Whether a turn may use a host or
//! run a program there is the command policy's (`CommandPolicyEvaluator`,
//! ADR 803 step 4), answered by Cedar (`infrastructure::tool_policy::cedar`),
//! which reads these lists and [`program_name`] as facts. A program is
//! matched by its file name, the part after its last `/`, so `/usr/bin/rm` is
//! `rm`. This is the gateway's tool policy, not a sandbox: an allowed program
//! can still run any other it is given as an argument.
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
}

/// A program's file name: what follows its last `/`, so `/usr/bin/rm` is
/// `rm`. A fact the command policy is read with, not a decision.
pub fn program_name(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

#[cfg(test)]
#[path = "../../../tests/conversation/command_policy.rs"]
mod tests;
