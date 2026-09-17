use crate::agents::domain::AgentId;

/// Why this machine could not answer a question about an agent.
///
/// Typed rather than a bare `false`, because "there is no sign-in" and "the
/// keychain is locked" are different facts that happen to look alike from the
/// outside. The adapter reports which one it met; deciding what to make of it
/// is not the adapter's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    /// There was nothing here to ask: no runtime directory to look in, no home
    /// directory, no keychain tool on this host.
    NothingToAsk,
    /// Something was there to ask and did not answer: an unreadable file, a
    /// locked keychain, a tool that failed.
    Unanswered,
}

/// What the host can be asked about an agent, and nothing more.
///
/// Two questions rather than one answer, so that the rule turning them into a
/// readiness lives in the domain — and so that an adapter cannot decide policy
/// by reporting a state directly.
pub trait AgentProbe: Send + Sync {
    /// Whether the agent's adapter is installed with this server.
    fn installed(&self, agent: AgentId) -> Result<bool, ProbeFailure>;

    /// Whether anything on this machine is signed in to the agent. Asked, never
    /// read: an implementation that has to handle the credential to answer is
    /// the wrong implementation.
    fn authenticated(&self, agent: AgentId) -> Result<bool, ProbeFailure>;
}
