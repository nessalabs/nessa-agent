use nessa_agent_credentials::{AgentCredential, CredentialAgent, CredentialNamespace};

use crate::agent_credentials::application::{AgentCredentialStore, CredentialStoreFailure};

/// A platform with no Nessa-owned secure credential store.
pub struct LocalAgentCredentialStore;

impl LocalAgentCredentialStore {
    /// Build the platform adapter. The namespace is retained only on platforms
    /// that have a credential store capable of using it.
    pub fn new(_: CredentialNamespace) -> Self {
        Self
    }
}

impl AgentCredentialStore for LocalAgentCredentialStore {
    fn save_api_key(
        &self,
        _: CredentialAgent,
        _: &AgentCredential,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unavailable)
    }
}
