//! Native device enrollment: an owner creates a one-time code, a device claims
//! it over TLS with a PAKE, and the owner approves the exact device key.
//!
//! Enrollment ends at Approved: nothing here stages a receiver or issues a
//! credential. Composition mounts the listener only when `config.json` names a
//! native listen address, and the product socket reaches the owner side through
//! `infrastructure::PairingOwnerCommands`; see the
//! [device pairing design](../../../../docs/design/auth/device-pairing.md).
//!
//! ```text
//! infrastructure (listener, connection, client) --> application --> Auth owners
//! infrastructure::enrollment_channel           --> wire + Auth TLS transport
//! ```
//! Arrows are compile-time dependencies. Auth is the only enrollment authority.
pub mod application;
pub mod infrastructure;
