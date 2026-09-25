//! Configures and opens a Claude harness context through shared ACP sessions.
//!
//! ```text
//! host config -> ClaudeAcpProvider -> profile -> ACP session
//!                                     |
//!                             Claude configuration checks
//! ```
//! Arrows show construction and validation. Process startup, resume, and the
//! launch-input fingerprint that identifies a restorable context remain shared
//! ACP responsibilities, not provider-specific lifecycle implementations.
//! `deletion` says what a successful `session/delete` means for Claude: the
//! adapter deletes the session file.
mod binding;
mod configuration;
mod deletion;
mod profile;
pub use binding::ClaudeAcpProvider;
