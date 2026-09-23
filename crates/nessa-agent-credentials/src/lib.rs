//! Shared domain language for credentials Nessa gives local coding agents.
//!
//! Credential adapters can share these validated values without moving
//! keychain names or operating-system effects into the domain.
//!
//! ```text
//! gateway source ──▶ AgentCredential
//!                          │
//!                          └── private validated text + credential kind
//! ```
//! Arrows mean construction or consumption of the immutable domain value.

pub mod domain;
pub use domain::value_objects::{
    AgentCredential, AgentCredentialError, AgentCredentialKind, CredentialAgent,
    CredentialNamespace, CredentialNamespaceError,
};
