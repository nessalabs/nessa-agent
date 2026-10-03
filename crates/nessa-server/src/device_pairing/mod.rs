//! Native device enrollment: an owner creates a one-time code, a device claims
//! it over TLS with a PAKE, and the owner approves the exact device key.
//!
//! This slice ends at Approved. It is not mounted in the default gateway, and
//! it stages no receiver and issues no credential; see the
//! [device pairing design](../../../../docs/design/auth/device-pairing.md).
//!
//! ```text
//! infrastructure (listener, connection, client) --> application --> Auth owners
//! infrastructure::enrollment_channel           --> wire + Auth TLS transport
//! ```
//! Arrows are compile-time dependencies. Auth is the only enrollment authority.
pub mod application;
pub mod infrastructure;
