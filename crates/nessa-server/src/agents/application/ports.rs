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
/// A consumer reads whenever it needs the current value. Composition that
/// requires launch and readiness to agree must inject this same source into
/// both consumers; the port itself does not select either consumer.
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

/// One internally coherent observation of an agent on this host.
pub struct AgentProbeEvidence {
    /// Whether the resolved launch was installed when this observation was made.
    pub installed: Result<bool, ProbeFailure>,
    /// Whether the launch had a supported credential, when it needs one.
    pub authenticated: Option<Result<bool, ProbeFailure>>,
}

/// What the host can be asked about an agent, and nothing more.
///
/// Separate fields in one observation keep the rule turning them into a
/// readiness in the domain, while preventing an adapter from deciding policy
/// by reporting a state directly.
pub trait AgentProbe: Send + Sync {
    /// Observe current launch and credential evidence for `agent`.
    ///
    /// `None` means this server has no configuration for the agent. Credential
    /// values never cross this port. A later decision takes a fresh observation
    /// because files and secure storage can change after this method returns.
    fn evidence(&self, agent: AgentId) -> Option<AgentProbeEvidence>;
}
