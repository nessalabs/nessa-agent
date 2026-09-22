//! ACP transport organized by the behavior it adapts.
//!
//! ```text
//! provider profile -> sessions -> executions worker -> application controller
//!                                      |       |
//!                                      v       v
//!                                    tools  permissions
//!                                      \       /
//!                                       JSON-RPC -> process
//! ```
//! Arrows show calls and data translation. Sessions own attachment and resume;
//! one execution worker serializes commands, observations, and review effects.
//! Tools and permissions translate wire values; they do not own domain rules.
pub(crate) mod executions;
pub(crate) mod fields;
pub(crate) mod permissions;
pub(crate) mod profile;
pub mod sessions;
pub(crate) mod tools;

// Kept in the shared tests tree, compiled internally for crate-private controls.
#[cfg(all(test, unix))]
#[path = "../../../tests/infrastructure/acp/mod.rs"]
mod tests;
