//! Asking this machine about the agents installed on it. Filesystem and
//! keychain effects live here; what the answers mean does not.
mod local;
pub use local::LocalAgentProbe;
