//! The narrow host-owned port for storing one validated agent API key.
mod ports;
pub use ports::{
    AgentCredentialStore, CredentialSaveAudit, CredentialSaveAuditFailure, CredentialSaveIds,
    CredentialSaveTargets, CredentialStoreFailure,
};
mod save_api_key;
pub use save_api_key::{save_api_key, CredentialSaveAdmissionFailure, CredentialSaveResult};
