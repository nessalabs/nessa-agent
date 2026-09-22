//! Platform adapters for the host's agent credential store.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::LocalAgentCredentialStore;
#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
pub use unsupported::LocalAgentCredentialStore;
