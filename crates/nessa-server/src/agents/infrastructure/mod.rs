//! Asking this machine about the agents installed on it. Filesystem and
//! keychain effects live here; what the answers mean does not.
//!
//! ```text
//!   local.rs  ──asks──▶  claude.rs
//!   (order of sources,   (Claude Code's own conventions: its keychain item,
//!    what an unanswered    its credentials file, its sign-in variables)
//!    source means)
//! ```
//!
//! The arrow points one way: `claude.rs` knows nothing about how its answers
//! are combined, and `local.rs` knows nothing about where Claude keeps a
//! sign-in. A second agent gets its own sibling module rather than a branch
//! inside this one.
mod claude;
mod local;
pub use local::LocalAgentProbe;
