//! What this host's owner grants each gateway that leases from it, and the
//! one decision made from it: whether a lease is admitted, and on what terms.
//!
//! ```text
//! Peer (ssh, or a dialed gateway's pin) ──▶ its PeerGrant, or none
//!   none ──▶ granted_to: ssh keeps everything, a dialed gateway nothing
//! admit_agent(grant, agent)        ──▶ Ok, or NotGranted
//! admit_command(grant, timeout_ms) ──▶ min(asked, maxTimeoutMs), or NotGranted
//! ```
//!
//! Arrows are what each step reads and answers. A grant only narrows what
//! the host can do: whether it runs an agent at all, or commands at all, is
//! its configuration's, decided before a grant is read. A grant never
//! widens a lease: a command's timeout comes back the one asked or shorter.
use nessa_sdk::domain::agent_execution::leases::{AgentWork, CommandWork};
use std::collections::BTreeSet;

/// Who is leasing from this host, as its grants name them. How a peer is
/// spelled in the grants file and on the command line is the file's
/// (`infrastructure::grants`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Peer {
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
pub(crate) enum Agents {
    /// Every agent the host runs.
    All,
    /// Only these, by their protocol names; none when empty.
    Named(BTreeSet<AgentName>),
}

/// An agent's protocol name, as a grant holds it: not blank, not `*`, and
/// no longer than a lease's agent may be.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct AgentName(Box<str>);

/// A value a grant cannot hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InvalidGrant {
    /// An agent name that is blank, `*`, or too long.
    AgentName,
    /// A command timeout of zero, or longer than any command may run.
    MaxTimeout,
}

impl AgentName {
    /// `name`, when a grant can hold it.
    ///
    /// # Errors
    /// [`InvalidGrant::AgentName`].
    pub(crate) fn new(name: &str) -> Result<Self, InvalidGrant> {
        if name.trim().is_empty() || name == "*" || name.len() > AgentWork::MAX_AGENT_BYTES {
            return Err(InvalidGrant::AgentName);
        }
        Ok(Self(name.into()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a peer may run commands for: at most this long each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommandGrant {
    max_timeout_ms: u64,
}

impl CommandGrant {
    /// Commands of at most `max_timeout_ms` each.
    ///
    /// # Errors
    /// [`InvalidGrant::MaxTimeout`] for zero, or past
    /// [`CommandWork::MAX_TIMEOUT_MS`].
    pub(crate) fn new(max_timeout_ms: u64) -> Result<Self, InvalidGrant> {
        if max_timeout_ms == 0 || max_timeout_ms > CommandWork::MAX_TIMEOUT_MS {
            return Err(InvalidGrant::MaxTimeout);
        }
        Ok(Self { max_timeout_ms })
    }

    pub(crate) fn max_timeout_ms(self) -> u64 {
        self.max_timeout_ms
    }
}

/// Everything one peer is granted here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerGrant {
    agents: Agents,
    /// `None`: no commands.
    commands: Option<CommandGrant>,
}

impl PeerGrant {
    pub(crate) fn new(agents: Agents, commands: Option<CommandGrant>) -> Self {
        Self { agents, commands }
    }

    /// No agent and no commands.
    pub(crate) fn nothing() -> Self {
        Self::new(Agents::Named(BTreeSet::new()), None)
    }

    /// Everything the host itself allows: every agent, and commands as long
    /// as any command may run.
    pub(crate) fn everything() -> Self {
        Self::new(
            Agents::All,
            Some(CommandGrant {
                max_timeout_ms: CommandWork::MAX_TIMEOUT_MS,
            }),
        )
    }

    pub(crate) fn agents(&self) -> &Agents {
        &self.agents
    }

    pub(crate) fn commands(&self) -> Option<CommandGrant> {
        self.commands
    }

    /// This grant and `agents` too: named ones join those already named,
    /// and [`Agents::All`] stays all.
    pub(crate) fn with_agents(self, agents: Agents) -> Self {
        let agents = match (self.agents, agents) {
            (Agents::All, _) | (_, Agents::All) => Agents::All,
            (Agents::Named(held), Agents::Named(added)) => {
                Agents::Named(held.union(&added).cloned().collect())
            }
        };
        Self { agents, ..self }
    }

    /// This grant with its commands replaced by `commands`.
    pub(crate) fn with_commands(self, commands: CommandGrant) -> Self {
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
    pub(crate) fn without_agent(self, agent: &AgentName) -> Result<Self, AllAgents> {
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
    pub(crate) fn without_commands(self) -> Self {
        Self {
            commands: None,
            ..self
        }
    }
}

/// A grant of every agent, from which one cannot be taken by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AllAgents;

/// What `peer` is granted, given its entry in the grants, if it has one. A
/// dialed gateway with none is granted nothing; SSH with none keeps what a
/// host served over SSH always had, everything its configuration allows.
pub(crate) fn granted_to(peer: &Peer, entry: Option<PeerGrant>) -> PeerGrant {
    match (entry, peer) {
        (Some(entry), _) => entry,
        (None, Peer::Ssh) => PeerGrant::everything(),
        (None, Peer::Gateway { .. }) => PeerGrant::nothing(),
    }
}

/// A lease the peer's grant does not cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NotGranted;

/// Whether `grant` covers a lease of `agent`'s harness.
///
/// # Errors
/// [`NotGranted`] when it does not.
pub(crate) fn admit_agent(grant: &PeerGrant, agent: &str) -> Result<(), NotGranted> {
    match &grant.agents {
        Agents::All => Ok(()),
        Agents::Named(named) if named.iter().any(|name| name.as_str() == agent) => Ok(()),
        Agents::Named(_) => Err(NotGranted),
    }
}

/// The timeout a command asking `timeout_ms` is granted under `grant`: the
/// one asked, or the grant's longest when that is shorter. Never longer
/// than asked.
///
/// # Errors
/// [`NotGranted`] when the grant holds no commands.
pub(crate) fn admit_command(grant: &PeerGrant, timeout_ms: u64) -> Result<u64, NotGranted> {
    let commands = grant.commands.ok_or(NotGranted)?;
    Ok(timeout_ms.min(commands.max_timeout_ms))
}

#[cfg(test)]
#[path = "../../../tests/env_serve/grants.rs"]
mod tests;
