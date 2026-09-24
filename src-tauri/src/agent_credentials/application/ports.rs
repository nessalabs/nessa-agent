use std::{error::Error, fmt};

use nessa_agent_credentials::{AgentCredential, CredentialAgent};

use crate::agent_credentials::domain::value_objects::{
    CredentialSaveCorrelation, CredentialSaveIntent, CredentialSaveOutcome, CredentialSaveTarget,
};

/// Writes Nessa-owned API keys without exposing a storage name to callers.
pub trait AgentCredentialStore: Send + Sync {
    /// Resolve the one canonical secure-store destination for an agent.
    fn target(
        &self,
        agent: CredentialAgent,
    ) -> Result<CredentialSaveTarget, CredentialStoreFailure>;

    /// Replace the API key for one explicitly supported local agent.
    ///
    /// # Errors
    /// Returns [`CredentialStoreFailure::Invalid`] if `credential` is not an
    /// API key, or [`CredentialStoreFailure::Unavailable`] if the platform
    /// store cannot durably replace the item.
    fn save_api_key(
        &self,
        target: &CredentialSaveTarget,
        credential: &AgentCredential,
    ) -> Result<(), CredentialStoreFailure>;
}

/// Resolves the canonical audited destination independently of the effect adapter.
pub trait CredentialSaveTargets: Send + Sync {
    fn target(
        &self,
        agent: CredentialAgent,
    ) -> Result<CredentialSaveTarget, CredentialStoreFailure>;
}

/// Generates request correlations without hiding an ambient randomness source.
pub trait CredentialSaveIds: Send + Sync {
    fn next(&self) -> Result<CredentialSaveCorrelation, CredentialSaveAuditFailure>;
}

/// Durably records credential replacement intent and outcome without secrets.
pub trait CredentialSaveAudit: Send + Sync {
    fn record_intent(
        &self,
        intent: &CredentialSaveIntent,
    ) -> Result<(), CredentialSaveAuditFailure>;
    fn record_outcome(
        &self,
        outcome: &CredentialSaveOutcome,
    ) -> Result<(), CredentialSaveAuditFailure>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialSaveAuditFailure;

impl fmt::Display for CredentialSaveAuditFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("credential save audit is unavailable")
    }
}

impl Error for CredentialSaveAuditFailure {}

/// Why the host did not save an agent API key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialStoreFailure {
    /// The value is not an API key supported by this writer.
    Invalid,
    /// The platform credential store did not confirm whether replacement occurred.
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
