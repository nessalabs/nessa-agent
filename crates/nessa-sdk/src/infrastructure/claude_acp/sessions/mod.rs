//! Configures and opens a Claude harness context through shared ACP sessions.
//!
//! ```text
//! host config -> ClaudeAcpProvider -> profile -> ACP session
//!                                     |
//!                               configuration checks / context fingerprint
//! ```
//! Arrows show construction and validation. Process startup and resume remain
//! shared ACP responsibilities, not provider-specific lifecycle implementations.
mod binding;
mod configuration;
mod identity;
mod profile;
pub use binding::ClaudeAcpProvider;
