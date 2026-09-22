//! Saving credentials that Nessa supplies to explicitly supported local agents.
//!
//! ```text
//! trusted host command ──▶ AgentCredentialStore ──▶ macOS login keychain
//!                              ▲
//!                    validated shared credential
//! ```
//! Arrows mean a validated value crossing an application-owned effect port.

pub mod application;
pub mod infrastructure;
