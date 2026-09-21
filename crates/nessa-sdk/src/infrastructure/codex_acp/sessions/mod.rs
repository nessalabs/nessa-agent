//! Configures and opens a Codex context through shared ACP sessions.
//!
//! ```text
//! host config -> CodexAcpProvider -> profile -> ACP session
//!                                     |
//!                          Codex selections and their checks
//! ```
//! Arrows show construction and validation. Process startup, resume, and the
//! launch-input fingerprint that identifies a restorable context remain shared
//! ACP responsibilities, not provider-specific lifecycle implementations.
mod binding;
mod profile;
pub use binding::CodexAcpProvider;
