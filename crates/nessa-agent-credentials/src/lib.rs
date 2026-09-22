//! Shared domain language for credentials Nessa gives local coding agents.
//!
//! Desktop and gateway adapters share these validated values without moving
//! keychain names or operating-system effects into the domain.
//!
//! ```text
//! desktop writer ──▶ AgentCredential ◀── gateway source
//!                         │
//!                         └── private validated text + credential kind
//! ```
//! Arrows mean construction or consumption of the immutable domain value.

pub mod domain;
pub use domain::value_objects::{
    AgentCredential, AgentCredentialError, AgentCredentialKind, CredentialAgent,
    CredentialNamespace, CredentialNamespaceError,
};
