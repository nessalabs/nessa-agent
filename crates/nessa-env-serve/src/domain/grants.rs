//! What this host's owner grants each gateway that leases from it, and the
//! terms a lease is admitted on once the lease policy allows it.
//!
//! ```text
//! Peer (ssh, or a dialed gateway's pin) ──▶ its PeerGrant, or none
//!   none ──▶ unwritten(peer): what it holds, shown as, changed from,
//!            and decided over by the lease policy
//! GrantOperation::apply(grant)        ──▶ the grant after an owner's change
//! lease_ms(grant)                     ──▶ a harness lease's longest, maxLeaseMs
//! command_timeout(grant, timeout_ms)  ──▶ min(asked, maxTimeoutMs)
//! ```
//!
//! Arrows are what each step reads and answers. Whether a peer may lease an
//! agent, or run a command, at all is not decided here: it is the lease
//! policy's (`application::LeasePolicyEvaluator`, answered by Cedar), over
//! the peer and the grant it holds. What is here are what a grant holds,
//! a missing entry's included, and the terms attached to a yes. A grant only narrows what the host can do: whether it runs an
//! agent at all, or commands at all, is its configuration's, decided after
//! the policy. A grant never widens a lease: a command's timeout comes back
//! the one asked or shorter.
use nessa_sdk::domain::agent_execution::leases::{AgentWork, CommandWork};
use std::collections::BTreeSet;

/// Who is leasing from this host, as its grants name them. How a peer is
/// spelled in the grants file and on the command line is the file's
/// (`infrastructure::grants`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Peer {
    /// Whoever runs `nessa env serve` over SSH: a gateway that logged in as
    /// this host's account, which no key here names.
    Ssh,
    /// The gateway a paired machine dials, by the pin its pairing keeps:
    /// the gateway's public key, 44 bytes of DER.
    Gateway {
        /// The gateway's key, as the pairing stores it.
        pin: [u8; 44],
    },
}

/// The agents a peer may lease harnesses for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Agents {
    /// Every agent the host runs.
    All,
    /// Only these, by their protocol names; none when empty.
    Named(BTreeSet<AgentName>),
}

/// An agent's protocol name, as a grant holds it: not blank, not `*`, and
/// no longer than a lease's agent may be.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentName(Box<str>);

/// A value a grant cannot hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidGrant {
    /// An agent name that is blank, `*`, or too long.
    AgentName,
    /// A command timeout of zero, or longer than any command may run.
    MaxTimeout,
    /// A lease duration of zero, or longer than [`MaxLease::MOST_MS`].
    MaxLease,
}

impl AgentName {
    /// `name`, when a grant can hold it.
    ///
    /// # Errors
    /// [`InvalidGrant::AgentName`].
    pub fn new(name: &str) -> Result<Self, InvalidGrant> {
        if name.trim().is_empty() || name == "*" || name.len() > AgentWork::MAX_AGENT_BYTES {
            return Err(InvalidGrant::AgentName);
        }
        Ok(Self(name.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a peer may run commands for: at most this long each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandGrant {
    max_timeout_ms: u64,
}

impl CommandGrant {
    /// Commands of at most `max_timeout_ms` each.
    ///
    /// # Errors
    /// [`InvalidGrant::MaxTimeout`] for zero, or past
    /// [`CommandWork::MAX_TIMEOUT_MS`].
    pub fn new(max_timeout_ms: u64) -> Result<Self, InvalidGrant> {
        if max_timeout_ms == 0 || max_timeout_ms > CommandWork::MAX_TIMEOUT_MS {
            return Err(InvalidGrant::MaxTimeout);
        }
        Ok(Self { max_timeout_ms })
    }

    pub fn max_timeout_ms(self) -> u64 {
        self.max_timeout_ms
    }
}

/// How long a harness's lease may last from its grant: past it, the host
/// ends the lease itself. A command lease is bounded by its own timeout
/// instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaxLease {
    ms: u64,
}

impl MaxLease {
    /// The longest a grant may let a lease last: thirty days.
    pub const MOST_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

    /// Leases of at most `ms` each.
    ///
    /// # Errors
    /// [`InvalidGrant::MaxLease`] for zero, or past [`Self::MOST_MS`].
    pub fn new(ms: u64) -> Result<Self, InvalidGrant> {
        if ms == 0 || ms > Self::MOST_MS {
            return Err(InvalidGrant::MaxLease);
        }
        Ok(Self { ms })
    }

    pub fn ms(self) -> u64 {
        self.ms
    }
}

/// Everything one peer is granted here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerGrant {
    agents: Agents,
    /// `None`: no commands.
    commands: Option<CommandGrant>,
    /// `None`: a harness's lease lasts until it is ended.
    max_lease: Option<MaxLease>,
    /// Whether a harness this peer starts may be given the peer's saved
    /// login for its agent ([`takes_login`]).
    logins: bool,
}

impl PeerGrant {
    /// `agents` and `commands`, its leases lasting until ended.
    pub fn new(agents: Agents, commands: Option<CommandGrant>) -> Self {
        Self {
            agents,
            commands,
            max_lease: None,
            logins: false,
        }
    }

    /// No agent and no commands.
    pub fn nothing() -> Self {
        Self::new(Agents::Named(BTreeSet::new()), None)
    }

    /// Everything the host itself allows: every agent, and commands as long
    /// as any command may run.
    pub fn everything() -> Self {
        Self::new(
            Agents::All,
            Some(CommandGrant {
                max_timeout_ms: CommandWork::MAX_TIMEOUT_MS,
            }),
        )
    }

    pub fn agents(&self) -> &Agents {
        &self.agents
    }

    pub fn commands(&self) -> Option<CommandGrant> {
        self.commands
    }

    pub fn max_lease(&self) -> Option<MaxLease> {
        self.max_lease
    }

    pub fn logins(&self) -> bool {
        self.logins
    }

    /// This grant, taking the peer's saved logins for its harnesses.
    pub fn with_logins(self) -> Self {
        Self {
            logins: true,
            ..self
        }
    }

    /// This grant, taking no login from the peer.
    pub fn without_logins(self) -> Self {
        Self {
            logins: false,
            ..self
        }
    }

    /// This grant with its harnesses' leases lasting at most `max_lease`,
    /// in place of what it had before.
    pub fn with_max_lease(self, max_lease: MaxLease) -> Self {
        Self {
            max_lease: Some(max_lease),
            ..self
        }
    }

    /// This grant and `agents` too: named ones join those already named,
    /// and [`Agents::All`] stays all.
    pub fn with_agents(self, agents: Agents) -> Self {
        let agents = match (self.agents, agents) {
            (Agents::All, _) | (_, Agents::All) => Agents::All,
            (Agents::Named(held), Agents::Named(added)) => {
                Agents::Named(held.union(&added).cloned().collect())
            }
        };
        Self { agents, ..self }
    }

    /// This grant with its commands replaced by `commands`.
    pub fn with_commands(self, commands: CommandGrant) -> Self {
        Self {
            commands: Some(commands),
            ..self
        }
    }

    /// This grant without `agent`.
    ///
    /// # Errors
    /// [`AllAgents`] when it grants every agent: one cannot be taken out of
    /// all of them by name.
    pub fn without_agent(self, agent: &AgentName) -> Result<Self, AllAgents> {
        match self.agents {
            Agents::All => Err(AllAgents),
            Agents::Named(mut named) => {
                named.remove(agent);
                Ok(Self {
                    agents: Agents::Named(named),
                    ..self
                })
            }
        }
    }

    /// This grant without commands.
    pub fn without_commands(self) -> Self {
        Self {
            commands: None,
            ..self
        }
    }
}

/// One change this host's owner makes to a peer's grant, as `nessa env
/// grant` and `nessa env ungrant` ask it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantOperation {
    /// `agents` beside those granted, commands of at most `commands` each
    /// in place of the longest granted before, and harnesses' leases of at
    /// most `max_lease` each in place of the limit before, and, when
    /// `logins`, the peer's saved logins taken for its harnesses.
    Grant {
        agents: Option<Agents>,
        commands: Option<CommandGrant>,
        max_lease: Option<MaxLease>,
        logins: bool,
    },
    /// One agent taken back.
    UngrantAgent(AgentName),
    /// Commands taken back.
    UngrantCommands,
    /// Saved logins no longer taken.
    UngrantLogins,
    /// Everything taken back.
    UngrantAll,
}

impl GrantOperation {
    /// `grant` after this change.
    ///
    /// # Errors
    /// [`AllAgents`] when one agent is taken by name from a grant of every
    /// agent.
    pub fn apply(&self, grant: PeerGrant) -> Result<PeerGrant, AllAgents> {
        Ok(match self {
            Self::Grant {
                agents,
                commands,
                max_lease,
                logins,
            } => {
                let grant = if *logins { grant.with_logins() } else { grant };
                let grant = match agents {
                    Some(agents) => grant.with_agents(agents.clone()),
                    None => grant,
                };
                let grant = match commands {
                    Some(commands) => grant.with_commands(*commands),
                    None => grant,
                };
                match max_lease {
                    Some(max_lease) => grant.with_max_lease(*max_lease),
                    None => grant,
                }
            }
            Self::UngrantAgent(agent) => grant.without_agent(agent)?,
            Self::UngrantCommands => grant.without_commands(),
            Self::UngrantLogins => grant.without_logins(),
            Self::UngrantAll => PeerGrant::nothing(),
        })
    }
}

/// A grant of every agent, from which one cannot be taken by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllAgents;

/// What a peer with no entry holds: everything the host allows for SSH,
/// its saved logins included (a gateway that logs in as this account can
/// already set anything it runs with), and nothing for a dialed gateway.
/// The one statement of a missing entry's
/// defaults, as a file's missing fields have theirs: it is what such a peer
/// is shown as and changed from, the grant the lease policy decides over,
/// and the terms of its yes. The policy holds no defaults of its own.
pub fn unwritten(peer: &Peer) -> PeerGrant {
    match peer {
        Peer::Ssh => PeerGrant::everything().with_logins(),
        Peer::Gateway { .. } => PeerGrant::nothing(),
    }
}

/// A lease the peer is not granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotGranted;

/// How long a harness's lease admitted under `grant` may last: `None`
/// until it is ended.
pub fn lease_ms(grant: &PeerGrant) -> Option<u64> {
    grant.max_lease.map(MaxLease::ms)
}

/// Whether a harness admitted under `grant` may be given its gateway's
/// saved login for its agent: what `Granted`'s `takesLogin` says.
pub fn takes_login(grant: &PeerGrant) -> bool {
    grant.logins
}

/// The timeout a command asking `timeout_ms` runs with under `grant`: the
/// one asked, or the grant's longest when that is shorter. Never longer
/// than asked. `None` when the grant holds no command terms to run it on.
pub fn command_timeout(grant: &PeerGrant, timeout_ms: u64) -> Option<u64> {
    grant
        .commands
        .map(|commands| timeout_ms.min(commands.max_timeout_ms))
}

#[cfg(test)]
#[path = "../../tests/grants.rs"]
mod tests;
