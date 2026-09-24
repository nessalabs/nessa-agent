//! Saving credentials that Nessa supplies to explicitly supported local agents.
//!
//! ```text
//! trusted host command ──▶ application::save_api_key
//!                               │
//!                   intent audit acknowledged
//!                               │
//!                               ▼
//!                   AgentCredentialStore effect
//!                               │
//!                               ▼
//!                      correlated outcome audit
//! ```
//! Arrows show the effect order owned by the application use case. The store,
//! correlation source and audit sink are application-owned ports supplied by
//! infrastructure at composition. The outcome keeps confirmed, refused and
//! uncertain store effects distinct from whether its audit delivery succeeded.

pub mod application;
pub mod domain;
pub mod infrastructure;
