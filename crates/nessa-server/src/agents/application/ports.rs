use crate::agents::domain::AgentId;
pub use nessa_agent_credentials::{AgentCredential, AgentCredentialKind};

/// Why a configured credential could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCredentialFailure {
    /// The credential store exists but did not answer safely.
    Unavailable,
    /// The stored value cannot be used as a process credential.
    Invalid,
}

/// Credentials Nessa may give an explicitly supported local agent.
///
/// Read for each launch so a credential saved during onboarding is immediately
/// usable. Readiness receives the same instance; it never infers sign-in from a
/// different store.
pub trait AgentCredentialSource: Send + Sync {
    /// Read the current credential for `agent`, or `None` when Nessa stores none.
    fn read(&self, agent: AgentId) -> Result<Option<AgentCredential>, AgentCredentialFailure>;
}

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
/// Separate questions rather than one answer, so that the rule turning them
/// into a readiness lives in the domain — and so that an adapter cannot decide
/// policy by reporting a state directly.
pub trait AgentProbe: Send + Sync {
    /// Whether this server has anything configured to launch for this agent.
    ///
    /// Answered from the configuration alone, so it never fails: the
    /// configuration is in memory and has already been read. Asked apart from
    /// [`Self::installed`] because "this build was not set up for it" and "it
    /// is not on this machine" are different facts, and only one of them is
    /// fixed by installing anything.
    fn configured(&self, agent: AgentId) -> bool;

    /// Whether the agent's adapter is installed with this server.
    fn installed(&self, agent: AgentId) -> Result<bool, ProbeFailure>;

    /// Whether anything on this machine is signed in to the agent. Asked, never
    /// read: an implementation that has to handle the credential to answer is
    /// the wrong implementation.
    fn authenticated(&self, agent: AgentId) -> Result<bool, ProbeFailure>;
}
