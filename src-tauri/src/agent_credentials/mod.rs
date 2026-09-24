//! Saving credentials that Nessa supplies to explicitly supported local agents.
//!
//! ```text
//! trusted host command ──▶ application::save_api_key
//!                               │
//!          CredentialSaveTargets canonical destination
//!                     ║ full-target match
//!          AgentCredentialStore claimed destination
//!                               │
//!                  correlation allocated
//!                               │
//!                   intent audit acknowledged
//!                               │
//!                               ▼
//!                   AgentCredentialStore effect
//!                               │
//!                               ▼
//!                      correlated outcome audit
//! ```
//! Arrows show the effect order owned by the application use case. The
//! independently injected `CredentialSaveTargets` authority derives the exact
//! canonical destination from the durable namespace. The application compares
//! every target field with the credential effect adapter's claim before it
//! allocates a correlation, records an intent, or touches the keychain. The
//! target authority, store, correlation source, and audit sink are
//! application-owned ports supplied by infrastructure at composition. The
//! outcome keeps confirmed, refused, and uncertain store effects distinct from
//! whether its audit delivery succeeded.

pub mod application;
pub mod domain;
pub mod infrastructure;
