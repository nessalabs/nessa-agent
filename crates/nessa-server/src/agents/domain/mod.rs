//! What an agent is, and what stands between it and running.

/// A coding agent Nessa knows by name.
///
/// Only agents Nessa has an adapter for are listed. An agent absent from here
/// is not one the server has an opinion about — it is one Nessa cannot drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentId {
    Claude,
}

impl AgentId {
    /// Every agent the server will report on.
    pub const ALL: &'static [AgentId] = &[AgentId::Claude];

    /// The name this agent is known by on the wire and in the interface.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentId::Claude => "claude",
        }
    }
}

/// What stands between an agent and running.
///
/// The three states are kept apart because they call for different things from
/// the person: nothing, a sign-in, or an install. Collapsing them into
/// "unavailable" would be easier to produce and useless to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Installed and signed in. The only state that may be offered.
    Ready,
    /// Installed, but nothing here is signed in to it.
    NeedsAuthentication,
    /// The adapter is not installed beside this server.
    NotInstalled,
}

impl Readiness {
    /// The name this state is known by on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Readiness::Ready => "ready",
            Readiness::NeedsAuthentication => "needs-authentication",
            Readiness::NotInstalled => "not-installed",
        }
    }
}
