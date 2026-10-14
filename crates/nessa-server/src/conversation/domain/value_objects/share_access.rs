//! What a share grant lets its holder do (issue 707): a role, and for Drive
//! the tool policy that holder's turns will run under.
//!
//! This value checks shape and size only. What a policy allows is decided
//! by the application's `ToolPolicyEvaluator`, which Cedar answers; this
//! value never matches a pattern itself.
//!
//! ```text
//! ShareAccess ──role──▶ ShareRole (Read, Comment or Drive)
//!      └──────policy──▶ ToolPolicy (Drive only) ──tools──▶ ToolPreset | ToolRules
//!                                  └──command_hosts──▶ CommandHosts
//! ```

use nessa_sdk::domain::agent_execution::leases::{EnvironmentRef, SshDestination};
use std::collections::BTreeSet;

/// How much a share lets its holder do. What each role allows is the
/// conversation access policy's, which Cedar answers; this value only names
/// the role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShareRole {
    /// Fold the records and watch them live.
    Read,
    /// Read, and leave notes the agent sees on the owner's next turn.
    Comment,
    /// Comment, and start, steer and stop turns under a tool policy.
    Drive,
}

/// A named tool policy a Drive grant may use instead of its own rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolPreset {
    /// Only tools that read.
    ReadOnlyTools,
    /// Every tool except shell commands.
    NoShell,
    /// Every tool, each one asked for.
    AskEverything,
    /// Every tool, asked as the conversation's approval mode asks.
    All,
}

/// Allow and deny patterns for the tools a Drive grant's turns may call. A
/// pattern is kept as written; what it matches is decided where the policy
/// is enforced, not here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRules {
    allow: Vec<String>,
    deny: Vec<String>,
}

impl ToolRules {
    /// The most patterns each list holds: allow, and separately deny.
    pub const MAX_RULES: usize = 16;
    /// The longest pattern, in UTF-8 bytes.
    pub const MAX_RULE_BYTES: usize = 48;

    /// Rules from `allow` and `deny`, refused when both are empty (a Drive
    /// grant's policy must say something), when either holds more than
    /// [`Self::MAX_RULES`] patterns, or when a pattern is empty, longer than
    /// [`Self::MAX_RULE_BYTES`], has surrounding whitespace, or holds a
    /// control character.
    pub fn new(allow: Vec<String>, deny: Vec<String>) -> Result<Self, ToolPolicyError> {
        if allow.is_empty() && deny.is_empty() {
            return Err(ToolPolicyError::NoRules);
        }
        if allow.len() > Self::MAX_RULES || deny.len() > Self::MAX_RULES {
            return Err(ToolPolicyError::TooManyRules);
        }
        for rule in allow.iter().chain(&deny) {
            if rule.is_empty() || rule.trim() != rule || rule.chars().any(char::is_control) {
                return Err(ToolPolicyError::MalformedRule);
            }
            if rule.len() > Self::MAX_RULE_BYTES {
                return Err(ToolPolicyError::RuleTooLong);
            }
        }
        Ok(Self { allow, deny })
    }

    /// The allow patterns, in the order they were given.
    pub fn allow(&self) -> &[String] {
        &self.allow
    }

    /// The deny patterns, in the order they were given.
    pub fn deny(&self) -> &[String] {
        &self.deny
    }
}

/// Which tools a Drive grant's turns may call: a preset, or rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolChoice {
    Preset(ToolPreset),
    Rules(ToolRules),
}

/// The hosts a Drive grant's turns may run commands on with `run`, by the
/// name `run` takes: an SSH destination or a paired environment's `peer:`
/// name. A grant that names none may use none, whatever the gateway is
/// configured with; whether a named host takes commands at all is still the
/// gateway's. Whether a host is used is the command policy's, which Cedar
/// answers; this value only checks the names' shape, length and number.
///
/// A host is at most [`Self::MAX_HOST_BYTES`], the longest SSH destination
/// `config.json` accepts ([`SshDestination::MAX_BYTES`]), and a policy names
/// at most [`Self::MAX_HOSTS`] of them, apart from its patterns. Whether one
/// page of grants still fits one `conversation.shares` frame is measured,
/// not argued: row J10's test builds a full page at that largest policy,
/// hosts and all, and checks it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandHosts(BTreeSet<String>);

impl CommandHosts {
    /// The most hosts one grant names.
    pub const MAX_HOSTS: usize = 16;
    /// The longest host a grant names, in bytes: the longest SSH destination
    /// `config.json` accepts. A `peer:` name is shorter.
    pub const MAX_HOST_BYTES: usize = SshDestination::MAX_BYTES;

    /// No hosts: a grant's turns run no commands.
    pub const fn none() -> Self {
        Self(BTreeSet::new())
    }

    /// `hosts`, refused when there are more than [`Self::MAX_HOSTS`], when
    /// one is longer than [`Self::MAX_HOST_BYTES`] or named twice, or when
    /// one is not a host `run` could name: an SSH destination or `peer:`
    /// name as `EnvironmentRef::named` reads it, never `here`, whose
    /// commands are the agent's own shell's.
    pub fn new(hosts: Vec<String>) -> Result<Self, ToolPolicyError> {
        if hosts.len() > Self::MAX_HOSTS {
            return Err(ToolPolicyError::TooManyHosts);
        }
        let mut named = BTreeSet::new();
        for host in hosts {
            if host.len() > Self::MAX_HOST_BYTES {
                return Err(ToolPolicyError::MalformedHost);
            }
            match EnvironmentRef::named(&host) {
                Ok(EnvironmentRef::Ssh(_) | EnvironmentRef::Peer(_)) => {}
                _ => return Err(ToolPolicyError::MalformedHost),
            }
            if !named.insert(host) {
                return Err(ToolPolicyError::MalformedHost);
            }
        }
        Ok(Self(named))
    }

    /// The hosts, in order.
    pub fn names(&self) -> &BTreeSet<String> {
        &self.0
    }

    /// Whether it names no host.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The policy a Drive grant's turns run under: which tools they may call,
/// and the hosts they may run commands on. Both are one fact of the grant,
/// changed together under one policy revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolPolicy {
    tools: ToolChoice,
    command_hosts: CommandHosts,
}

impl ToolPolicy {
    /// `preset`, on no command hosts.
    pub const fn preset(preset: ToolPreset) -> Self {
        Self {
            tools: ToolChoice::Preset(preset),
            command_hosts: CommandHosts::none(),
        }
    }

    /// `rules`, on no command hosts.
    pub fn rules(rules: ToolRules) -> Self {
        Self {
            tools: ToolChoice::Rules(rules),
            command_hosts: CommandHosts::none(),
        }
    }

    /// This policy, with its turns' commands run on `hosts` only. Hosts are
    /// not counted against its patterns: both lists may be full and `hosts`
    /// may still name [`CommandHosts::MAX_HOSTS`].
    pub fn with_command_hosts(self, hosts: CommandHosts) -> Self {
        Self {
            command_hosts: hosts,
            ..self
        }
    }

    /// Which tools its turns may call.
    pub fn tools(&self) -> &ToolChoice {
        &self.tools
    }

    /// The hosts its turns may run commands on.
    pub fn command_hosts(&self) -> &CommandHosts {
        &self.command_hosts
    }
}

/// Why a tool policy was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolPolicyError {
    /// Neither an allow nor a deny pattern.
    NoRules,
    /// More than [`ToolRules::MAX_RULES`] patterns in allow, or in deny.
    TooManyRules,
    /// A pattern longer than [`ToolRules::MAX_RULE_BYTES`].
    RuleTooLong,
    /// An empty pattern, one with surrounding whitespace, or one holding a
    /// control character.
    MalformedRule,
    /// More than [`CommandHosts::MAX_HOSTS`] command hosts.
    TooManyHosts,
    /// A command host that is no SSH destination or `peer:` name, `here`,
    /// one longer than [`CommandHosts::MAX_HOST_BYTES`], or one named twice.
    MalformedHost,
}

/// A role with the policy it needs: a Drive grant always has a tool policy,
/// and a Read or Comment grant never has one, because neither starts a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareAccess {
    role: ShareRole,
    policy: Option<ToolPolicy>,
}

impl ShareAccess {
    /// `role` with `policy`, refused when a Drive grant names no policy or
    /// a Read or Comment grant names one.
    pub fn new(role: ShareRole, policy: Option<ToolPolicy>) -> Result<Self, ShareAccessError> {
        match (role, &policy) {
            (ShareRole::Drive, None) => Err(ShareAccessError::PolicyRequired),
            (ShareRole::Read | ShareRole::Comment, Some(_)) => {
                Err(ShareAccessError::PolicyNotAllowed)
            }
            _ => Ok(Self { role, policy }),
        }
    }

    /// Read, which takes no policy.
    pub fn read() -> Self {
        Self {
            role: ShareRole::Read,
            policy: None,
        }
    }

    pub fn role(&self) -> ShareRole {
        self.role
    }

    /// The tool policy, present exactly when the role is Drive.
    pub fn policy(&self) -> Option<&ToolPolicy> {
        self.policy.as_ref()
    }
}

/// Why a role and policy were refused together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareAccessError {
    /// Drive names no tool policy.
    PolicyRequired,
    /// Read or Comment names a tool policy.
    PolicyNotAllowed,
}

#[cfg(test)]
#[path = "../../../../tests/conversation/share_access.rs"]
mod tests;
