//! Test-only profile substitution and raw ACP transport contracts.
//! Files live in the shared tests tree and are included by the ACP module only
//! for library tests, allowing access to crate-private controls. Public Agent usage
//! remains covered by application integration tests.
//!
//! ```text
//! independent test profile -> real ACP runtime -> Python test handler
//! ```
//! Arrows show execution through the adapter seam without model calls.
pub(super) mod profile_substitution;

#[cfg(unix)]
mod contracts;
