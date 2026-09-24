//! Platform adapters for the host's agent credential store.
mod commands;
pub use commands::{
    __cmd__save_agent_api_key, __tauri_command_name_save_agent_api_key, save_agent_api_key,
};
mod audit;
pub use audit::{FileCredentialSaveAudit, RandomCredentialSaveIds, UnavailableCredentialSaveAudit};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::LocalAgentCredentialStore;
#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
pub use unsupported::LocalAgentCredentialStore;
