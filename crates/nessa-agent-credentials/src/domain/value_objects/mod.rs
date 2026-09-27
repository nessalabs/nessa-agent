//! Immutable values for one supported local agent credential.
mod agent_credential;
pub use agent_credential::{AgentCredential, AgentCredentialError, AgentCredentialKind};
mod credential_agent;
pub use credential_agent::{CredentialAgent, CredentialNamespace, CredentialNamespaceError};
