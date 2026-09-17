//! Reading an agent's readiness, through a port the adapters fill in.

use crate::agents::domain::{AgentId, Readiness};

/// What the host can be asked about an agent, and nothing more.
///
/// Two questions rather than one answer, so that the rule turning them into a
/// readiness lives here and can be read in one place — and so that an adapter
/// cannot decide policy by reporting a state directly.
pub trait AgentProbe: Send + Sync {
    /// Whether the agent's adapter is installed with this server.
    fn installed(&self, agent: AgentId) -> bool;

    /// Whether anything on this machine is signed in to the agent. Asked, never
    /// read: an implementation that has to handle the credential to answer is
    /// the wrong implementation.
    fn authenticated(&self, agent: AgentId) -> bool;
}

/// Report what stands between each agent and running.
pub struct ReadAgentReadiness<'a> {
    /// The host being asked.
    pub probe: &'a dyn AgentProbe,
}

impl ReadAgentReadiness<'_> {
    /// An agent nobody is signed in to is not offered, and neither is one that
    /// is not there. Not installed is reported first because it is the reason
    /// signing in would not help.
    pub fn execute(&self, agent: AgentId) -> Readiness {
        if !self.probe.installed(agent) {
            return Readiness::NotInstalled;
        }
        if self.probe.authenticated(agent) {
            Readiness::Ready
        } else {
            Readiness::NeedsAuthentication
        }
    }

    /// Every agent the server reports on, in listing order.
    pub fn all(&self) -> Vec<(AgentId, Readiness)> {
        AgentId::ALL
            .iter()
            .map(|agent| (*agent, self.execute(*agent)))
            .collect()
    }
}

#[cfg(test)]
#[path = "../../../tests/agents/readiness.rs"]
mod tests;
