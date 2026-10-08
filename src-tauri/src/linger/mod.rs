//! Whether Linux setup keeps the gateway running after logout.
//!
//! Linger is an account policy on logind, not a step of registering the
//! systemd user unit. The unit runs while the person is signed in either way
//! ([ADR 217](../../../docs/adr/done/217-linux-linger-at-setup.md)).
//!
//! ```text
//! setup command ──▶ blocking pool ──▶ LingerOffer
//!                                       ├─ show() ─────────▶ the only logged-out claim
//!                                       └─ LogindLinger ──▶ read Linger, or SetUserLinger
//! ```
//!
//! Arrows are calls. `show` is pure. `SetUserLinger` is reached only from an
//! explicit accept while the read just taken says linger is off. For this
//! process's own uid that call is `set-self-linger`, which stock systemd allows
//! without an administrator.

pub(crate) mod application;
pub(crate) mod domain;
pub(crate) mod infrastructure;
