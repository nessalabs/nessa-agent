/// A coding agent Nessa knows by name.
///
/// Only agents Nessa has an adapter for are listed. An agent absent from here
/// is not one the server has an opinion about — it is one Nessa cannot drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentId {
    Claude,
    Codex,
}

impl AgentId {
    /// Every agent the server will report on, in listing order.
    pub const ALL: &'static [AgentId] = &[AgentId::Claude, AgentId::Codex];

    /// The one name this agent is known by outside the server.
    ///
    /// Configuration, the readiness route, the socket, and the conversation
    /// records on disk all name agents, and they must all name them the same
    /// way: a conversation stored as `codex` has to be the agent setup offered
    /// and the launcher starts. That makes the name part of the identity rather
    /// than a per-boundary spelling, which is why it lives here. Changing one is
    /// a change to every boundary at once, including records already written.
    pub fn name(self) -> &'static str {
        match self {
            AgentId::Claude => "claude",
            AgentId::Codex => "codex",
        }
    }

    /// The agent that name belongs to, or nothing for a name this server has no
    /// adapter for. Unknown is never resolved to a default: a configuration or a
    /// stored record naming an agent Nessa cannot drive is a fact to report, not
    /// one to quietly substitute another agent for.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|agent| agent.name() == name)
    }
}

#[cfg(test)]
#[path = "../../../../tests/agents/agent_id.rs"]
mod tests;
