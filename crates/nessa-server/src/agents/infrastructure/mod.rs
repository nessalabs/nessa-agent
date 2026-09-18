//! Asking this machine about the agents installed on it. Filesystem and
//! keychain effects live here; what the answers mean does not.
//!
//! ```text
//!   local.rs  ──asks──▶  claude.rs ─┐
//!   (order of sources,   codex.rs  ─┴─▶ credentials.rs
//!    what an unanswered   (each vendor's own      (what a credential
//!    source means)         conventions)            variable and file are)
//! ```
//!
//! The arrows point one way: an agent's module knows nothing about how its
//! answers are combined, and `local.rs` knows nothing about where any agent
//! keeps a sign-in. A third agent gets its own sibling module rather than a
//! branch inside an existing one.
mod claude;
mod codex;
mod credentials;
mod local;
pub use local::{AgentLaunchFiles, LocalAgentProbe};
