//! Whether Linux setup keeps the gateway running after logout.
//!
//! Linger is an account policy on logind, not a step of registering the
//! systemd user unit. The unit runs while the person is signed in either way
//! ([ADR 217](../../../docs/adr/todo/217-linux-linger-at-setup.md)).
//!
//! ```text
//! setup command ──▶ LingerOffer (this process's attempt)
//!                       │
//!                       ├─ show() ──────────▶ the only logged-out claim
//!                       ├─ LogindLinger ───▶ read Linger, or SetUserLinger
//!                       └─ LingerAudit ────▶ intent before the call, outcome after
//! ```
//!
//! Arrows are calls. `show` is pure. The attempt is memory of this process:
//! a new process starts with no attempt and can claim logged-out operation
//! only from a fresh read. `SetUserLinger` is reached only from an explicit
//! accept while that read says linger is off.

pub(crate) mod application;
pub(crate) mod domain;
