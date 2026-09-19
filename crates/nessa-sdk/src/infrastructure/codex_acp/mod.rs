//! Codex provider configuration and tool translation for the shared ACP runtime.
//!
//! ```text
//! sessions::CodexAcpProvider -> Codex profile -> shared ACP sessions
//!                                   |
//!                             tools wire mapping
//! ```
//! Arrows show construction and calls. Shared ACP owns execution and permission
//! transport; this provider module supplies the configuration Codex opens a
//! session from, the checks that confirm it took it, and the translation of
//! Codex's own tool frames.
//!
//! Codex differs from a harness whose tool set the host configures: it brings
//! its own shell and patch tools, runs them in a sandbox it owns, and asks only
//! when an action leaves that sandbox. This module does not pretend otherwise.
//! It pins the least permissive preset Codex offers, so that everything leaving
//! the sandbox is answered by Nessa's permission owner, and carries everything
//! Codex reports about the rest through as observations.
pub mod sessions;
pub(crate) mod tools;
