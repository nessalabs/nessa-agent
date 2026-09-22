//! Asking this machine about the agents installed on it. Filesystem and
//! keychain effects live here; what the answers mean does not.
//!
//! ```text
//!   local.rs  ──asks──▶  claude.rs ─┐
//!      │  (order of sources,  codex.rs ─┴─▶ credentials.rs
//!      │   what an unanswered  (each vendor's      (what a credential
//!      │   source means)        own conventions)    variable and file are)
//!      └──────────────reads a named file through────────▶
//! ```
//!
//! The arrows point one way: an agent's module knows nothing about how its
//! answers are combined, and `local.rs` knows nothing about where any agent
//! keeps a sign-in — it is handed the path each vendor module resolved, and
//! reads it through the same shared rule they all answer by, which is the
//! second edge above. A third agent gets its own sibling module rather than a
//! branch inside an existing one.
mod agent_credentials;
mod claude;
mod codex;
mod credentials;
mod local;
pub use agent_credentials::LocalAgentCredentials;
pub use local::{AgentLaunchFiles, LocalAgentProbe};
