//! Shared domain language for credentials Nessa gives local coding agents.
//!
//! The desktop host writes one validated value and the gateway reads that same
//! value for readiness and launch. Keychain names and operating-system effects
//! stay in each caller's infrastructure.
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
