use std::{error::Error, fmt};

use nessa_agent_credentials::{AgentCredential, CredentialAgent};

/// Writes Nessa-owned API keys without exposing a storage name to callers.
pub trait AgentCredentialStore: Send + Sync {
    /// Replace the API key for one explicitly supported local agent.
    ///
    /// # Errors
    /// Returns [`CredentialStoreFailure::Invalid`] if `credential` is not an
    /// API key, or [`CredentialStoreFailure::Unavailable`] if the platform
    /// store cannot durably replace the item.
    fn save_api_key(
        &self,
        agent: CredentialAgent,
        credential: &AgentCredential,
    ) -> Result<(), CredentialStoreFailure>;
}

/// Why the host did not save an agent API key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialStoreFailure {
    /// The value is not an API key supported by this writer.
    Invalid,
    /// The platform credential store did not confirm the replacement.
    Unavailable,
}

impl fmt::Display for CredentialStoreFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "the credential is not a supported API key",
            Self::Unavailable => "the system credential store is unavailable",
        })
    }
}

impl Error for CredentialStoreFailure {}
