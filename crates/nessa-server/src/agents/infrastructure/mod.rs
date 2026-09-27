//! Asking this machine about the agents installed on it. Filesystem and
//! keychain effects live here; what the answers mean does not.
//!
//! ```text
//!   local.rs  ──asks──▶  claude.rs ─┐
//!      │  (order of sources,  codex.rs ─┴─▶ credentials.rs
//!      │   what an unanswered  (each vendor's      (what a credential
//!      │   source means)        own conventions)    variable and file are)
//!      └──────────────reads a named file through────────▶
//!   credentialed_claude.rs ──reads──▶ agent_credentials.rs ──▶ keychain
//! ```
//!
//! The arrows point one way: an agent's module knows nothing about how its
//! answers are combined, and `local.rs` knows nothing about where any agent
//! keeps a sign-in — it is handed the path each vendor module resolved, and
//! reads it through the same shared rule they all answer by, which is the
//! second edge above. A third agent gets its own sibling module rather than a
//! branch inside an existing one.
//! The lower edge is separate from vendor sign-in discovery: it defines a
//! Nessa-owned source and Claude provider adapter that composition can inject.
//! Its macOS reader refuses any read that would require keychain UI.
mod agent_credentials;
#[cfg(target_os = "macos")]
mod agent_credentials_macos;
mod claude;
mod codex;
#[cfg(unix)]
mod credentialed_claude;
mod credentials;
mod local;
pub use agent_credentials::LocalAgentCredentials;
#[cfg(unix)]
pub use credentialed_claude::CredentialedClaudeProvider;
pub use local::{AgentLaunchFiles, LocalAgentProbe};
