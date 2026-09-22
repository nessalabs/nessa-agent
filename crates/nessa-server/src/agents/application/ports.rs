use crate::agents::domain::AgentId;

/// Which vendor credential Nessa obtained for an agent.
///
/// The distinction is part of launch behavior: Claude Code accepts both names,
/// but an OAuth token passed as an API key is not the credential the user gave
/// us. The secret itself deliberately implements neither `Debug` nor `Display`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCredentialKind {
    /// A provider API key.
    ApiKey,
    /// A vendor OAuth token.
    OAuthToken,
}

/// One credential read for the next agent launch.
///
/// The text stays private and is never serialized or used as diagnostic text.
/// The operating system, allocator, and provider APIs can retain transient
/// memory copies, so this value does not claim secure erasure.
pub struct AgentCredential {
    kind: AgentCredentialKind,
    secret: String,
}

impl AgentCredential {
    /// Build a bounded, nonempty credential.
    ///
    /// # Errors
    /// Returns [`AgentCredentialFailure::Invalid`] for a value that is empty,
    /// larger than 16 KiB, not UTF-8, or contains control characters. Such a
    /// value cannot be represented consistently at every provider boundary.
    pub fn new(kind: AgentCredentialKind, secret: Vec<u8>) -> Result<Self, AgentCredentialFailure> {
        if secret.is_empty() || secret.len() > 16 * 1024 {
            return Err(AgentCredentialFailure::Invalid);
        }
        let secret = String::from_utf8(secret).map_err(|_| AgentCredentialFailure::Invalid)?;
        if secret.chars().all(char::is_whitespace) || secret.chars().any(char::is_control) {
            return Err(AgentCredentialFailure::Invalid);
        }
        Ok(Self { kind, secret })
    }

    /// The environment meaning the provider adapter must preserve.
    pub fn kind(&self) -> AgentCredentialKind {
        self.kind
    }

    /// Borrow the secret only at the process-launch boundary.
    pub fn expose(&self) -> &str {
        self.secret.as_str()
    }
}

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

#[cfg(test)]
mod credential_tests {
    use super::*;

    #[test]
    fn credentials_accept_only_bounded_plain_utf8_text() {
        for secret in [
            Vec::new(),
            b"line\nbreak".to_vec(),
            b"   ".to_vec(),
            vec![0xff],
        ] {
            assert_eq!(
                AgentCredential::new(AgentCredentialKind::ApiKey, secret).err(),
                Some(AgentCredentialFailure::Invalid)
            );
        }
        assert_eq!(
            AgentCredential::new(AgentCredentialKind::ApiKey, vec![b'x'; 16 * 1024 + 1]).err(),
            Some(AgentCredentialFailure::Invalid)
        );
        let credential =
            AgentCredential::new(AgentCredentialKind::OAuthToken, b"token".to_vec()).unwrap();
        assert_eq!(credential.kind(), AgentCredentialKind::OAuthToken);
        assert_eq!(credential.expose(), "token");
    }
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
